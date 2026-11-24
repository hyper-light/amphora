# Embedded local owner transport (CP-T1.5, U05/U06)

The canonical database owns one engine, controller and durability pipeline. A second process
uses a product-owned local client; it never opens another engine. Database startup selects the
owner or client automatically and retains the listener in shared ownership. The listener
holds a weak-upgrade factory, not a strong Database reference, so individual clone drops cannot
stop it and the last handle can release ownership. Connections retain the database only for their
bounded lifetime. This module is not an installed daemon.

All supported platforms use the same endpoint: IPv4 loopback on an ephemeral port, a protected
companion directory and a fixed 34-byte port/capability record. A separate OS-locked private
lockfile serializes publication. private-fs creates private Unix modes and protected Windows
current-user-only DACLs, verifies access and retained handle identity, and refuses links/reparse
points. It also verifies macOS extended ACLs, which ordinary mode checks cannot establish.
The record is atomically replaced under the lock and retained after shutdown as one bounded
stale record. It contains no remotely routable address. The lockfile is separate because a
Windows exclusive byte-range lock also denies other processes' reads.

An additional private `database-owner` lease is acquired before opening the engine or creating
SQL state. The engine, flusher and readers retain the lease until their actual drain. Existing
hard-linked database files and dangling symlinks are refused; an OS-file-identity registry also
reuses an existing same-process owner for aliases. The embedding host must protect its database
directory namespace. In particular, private-fs validates the final Windows sidecar DACL and
reparse traversal, but does not validate ancestor DELETE_CHILD grants; this protocol does not
make an attacker-writable database directory safe.

Windows binds SO_EXCLUSIVEADDRUSE before bind through socket2's safe API; Linux/macOS do not
request reuse of an already bound listening endpoint. The exact upstream socket2 revision is
pinned because the safe Windows exclusivity setter merged after its latest published release.
There are no product unsafe blocks or production subprocesses.

The client authenticates the owner before revealing a client proof: random client/server nonces
and direction-separated HMAC-SHA256 proofs bind protocol version, nonces and requested lifetime.
The capability never crosses the socket. Thus a stale record whose port was reused by another
process cannot authenticate that process or expose the capability. Windows exclusive binding
prevents another local socket from intercepting or relaying traffic after the handshake. OS-local
stream isolation protects subsequent SQL/Records frames; this is not a network transport.
Between machines a node's endpoint listens at a network address under mutual TLS instead
(transport/network.rs; reshard.md, "The transport between machines").

The wire is a four-byte big-endian length followed by a postcard envelope. Variant order is
append-only. Frames are limited to RESULT_MAX plus 1 MiB of serialization overhead. Serialization
grows a buffer only up to that fixed cap; small control messages keep small allocations. Reads and
writes reset their kernel timeout to the remaining absolute deadline after every partial transfer.
All platforms use TcpStream::connect_timeout. Backlog saturation refuses before submission.
No retry loop replays writes or closures.

Before a new application operation, a client checks owner identity. If the old process has
exited, the client acquires the same exclusive lease, reopens the existing durable data through
its original local or replicated factory, and publishes the replacement endpoint. All clones
see that promoted owner. Authenticated identity includes the complete replication reopening
configuration; an explicitly configured attachment must match it. Identity probes may retry;
an application request that loses its response returns `Refusal::UnknownOutcome` and is never
replayed. `attach_replicated` lets a host attach before attempting exclusive bootstrap inspection;
`ensure_owner` lets its carrier coordinator discover local takeover before advertising a carrier.

Resource limits are admission policy, not measured tuning claims: connections bounded by the
process's descriptor budget (below, "One descriptor budget"; 32 a database until 2026-11-21), at most eight
Records waits, 30 seconds total per authenticated connection, two seconds to authenticate,
4096 commands per adaptive session, 256 bytes per authentication frame, and at most 33 MiB per
data frame. Collection decoders bound statement/parameter/transaction arrays before allocation
and charge SQL result collection overhead as well as value payload. Overload closes an unadmitted
connection; the client returns a typed connection refusal before submission. The persistent
accept loop checks shutdown every 10 ms (bounded idle shutdown latency); each admitted worker
releases its permit on every exit. Waits use existing Namespace waiting outside the SQL writer;
they cannot consume all connection slots. Frame allocation caps bound each connection's
transport memory to bounded wire and decoded values; the connections' sum is not yet charged to a
memory budget (below, "One descriptor budget", what is left). Authentication frames have a separate 256-byte cap.

Adaptive SQL is executed inside existing Database.call. The client closure runs once. Intermediate
query responses are provisional; Commit ends the closure and the final Done waits until the
real durable callback. Socket loss, deadline expiry, exhausted command budget or an explicit
Abort before Commit causes the closure to fail and the existing pipeline to roll back. Loss of
the final response after Commit is an unknown outcome; callers must not replay FnOnce. Records
CAS, SQL batches and migrations route directly to existing Database/Namespace methods.

SQL-session Records get/CAS commands also run inside the same Database.call, with provisional
revisions and typed refusals. Namespace import pages, progress and readiness route existing owner
methods. Remote async watches use one process-wide reactor with at most 4096 registrations,
64 metadata checks per request, and a 64 KiB reply limit before decoding. Each registration has
a thirty-second deadline and a 4096-probe limit. A metadata batch uses at most one connection,
does not enter the SQL writer, and consumes no synchronous-wait slot. Ten milliseconds between
checks is explicit notification policy, not a throughput claim. Polling only inspects a local
result/waker. Dropping a registration releases admission immediately; dropping the last
subscriber of an active batch interrupts that socket. Read-only metadata probes may retry
transient connection failures within those bounds. Synchronous Records waits remain limited
to eight; their owner-side watches check cancellation at most every 10 ms.

The local writer checks admission deadlines before invoking an application closure and installs
the existing SQL progress guard during application evaluation, with a check every 1024 VM
instructions. It drops the guard before transaction cleanup and durability work. The replicated
actor owns its corresponding evaluation guard. A trusted closure's arbitrary blocking host code
cannot be preempted; expiration after submission therefore remains an unknown outcome. Producer
result accounting charges owned-value overhead as well as payload before commit.

Falsification: a second independent engine acquiring the same canonical database; a successful
response before durable publication; a disconnected adaptive session leaving rows; one clone's
drop stopping another; an unauthenticated client reaching SQL; a Records wait preventing a SQL
client's admission; or any attacker-controlled allocation beyond the frame/API budgets.
`tests/transport.rs` exercises independent-process ownership, CAS, adaptive SQL, application rollback
and disconnect rollback. It also exercises an existing client becoming the next standalone and
single-node replicated owner after a predecessor process exits, retaining SQL rows and ledger
revisions. Endpoint tests check private record reading, competing owner refusal, protected
pre-bootstrap leases and hard-link refusal. Builds and behavior tests must run
on each supported target before claiming validation.

Primary evidence: [Rust File locking](https://doc.rust-lang.org/std/fs/struct.File.html#method.try_lock)
documents exclusive nonblocking file locking and release on handle closure. These references were
checked 2026-10-29. This design does not claim to stop a malicious process running as the same uid,
which already has access to the database files and capability.

Windows evidence: [Microsoft Winsock socket exclusivity](https://learn.microsoft.com/en-us/windows/win32/winsock/using-so-reuseaddr-and-so-exclusiveaddruse)
explains port takeover with SO_REUSEADDR and the protection afforded by SO_EXCLUSIVEADDRUSE.
[private-fs's security contract](https://github.com/orielhaim/private-fs/blob/main/SECURITY.md) and
[0.2.1 API](https://docs.rs/private-fs/0.2.1/private_fs/) describe protected user-only DACL creation,
handle-relative operations, validation and refusal semantics. [RFC 2104](https://www.rfc-editor.org/rfc/rfc2104.html)
specifies the HMAC construction; direction/domain labels and nonce binding are this protocol's
choice, falsified by a reflected or replayed proof authenticating a new connection.

## Versions (2026-11-06)

The greeting carries the protocol version (`wire::VERSION`), and the owner refuses a peer with no
version in common at once ("Negotiated versions"), `Invalid`, naming both versions, before a challenge and before any frame
whose shape differs. Postcard is not self-describing: a field added to a message is not filled by
`#[serde(default)]` on decode, so a client reading an older owner's longer or shorter record gets a
decode error mid-session, `Unavailable`, not the default. So every change to any message's fields
bumps the version. Before version 9 an owner and its clients changed version together across a drain;
since 9 they negotiate ("Negotiated versions"). Nothing decodes another version's records.
The owner's refusal is its own result, never a frame: it writes nothing and closes the connection,
so a client of another version sees the connection closed before any challenge, whatever shape its
frames have.

Version 4: the owner's `Identity` gained `durability` (replication.md, "Fates and durability
checked"). Version 3: `ReplicaStatus` gained `voters_outgoing` (replication.md, "Membership"). Version 2 had
already added `bootstrapping` without a bump; a version-2 client of an owner with that field would
have failed the same way. The test: `a_peer_of_another_version_is_refused_at_the_greeting`.

Version 5: `Request` gained `Application` (the application floor, replication.md), a client's bound
claim sent once after the greeting and before its ordinary request. The owner validates its shape and
answers Ready; only the owner's own floor, checked inside the writer's transaction, grants admission.

Version 6: `Request` gained `LocalSession` (replication.md, "A local read on a replica"), a session
the owner runs read-only on its own state, asking no leader. A client asks it without probing the
owner's capabilities: every owner it can greet takes it. No capability is advertised for it, and the
value 2 (bit 1), which kiln's 0003 gave `CAP_LOCAL_READ`, stays unassigned.

Version 7: `Request` gained `Pull` and `Answer` gained `Pulled` (reshard.md, decision "Pulls over the
owner transport"), a node's half of a copy or catch-up batch for a move's new group on another node.
A node's endpoint (`transport::Node`) answers it without a database; a database's owner answers it
`Pulled::NoGroup`.

Evidence: postcard's wire format: "A struct is encoded as the elements that comprise it, in their
order of definition", and "their length and field names are not encoded on the wire"
([postcard's wire format, section 28](https://postcard.jamesmunns.com/wire-format.html)), so a
decoder can't tell a missing trailing field from a short frame. Falsified by: a session that decodes a
record of another version, or a refusal of another version that comes after the greeting.

## Negotiated versions (2026-11-22)

Version 9 lets nodes of two versions talk, so a cluster upgrades one node at a time instead of
across a drain. Each side says the versions it speaks and both use the newest one in common:

- The client's greeting frame holds its `Hello`, whose `version` is now the oldest version it
  speaks, followed by `Speaks`, the newest. The owner's `Challenge` frame is followed by `Agreed`,
  the version it chose. Postcard decodes a frame's leading value and ignores what comes after it,
  so a version-8 peer reads these frames as its own and leaves the added value unread.
- A version-8 owner takes only a `Hello` of exactly 8. A client of this version says 8 as its
  oldest, so that owner takes it; the client then finds no `Agreed` and uses 8.
- No version in common is refused at the greeting, `Invalid`, naming both ranges.
- Once a side says what it speaks (the client sent `Speaks`, the owner sent `Agreed`), both proofs
  bind the client's newest and the chosen version (`auth::Agreement`). A greeting with one of the
  two cut off on the way then fails authentication instead of quietly agreeing on 8. Cutting both
  looks exactly like a version-8 peer, which no scheme a version-8 peer accepts can tell apart.
- Messages keep their shape at every version. A version adds variants after every existing one,
  and a variant is sent only on a connection that agreed on a version that has it: a request
  (`Request::since`, which names every request and every `xgroup::Step`) is refused before it goes
  out, `Invalid`, on a connection that agreed on an older version. Version 9's
  `Answer::SteppedClosed` is `Stepped` plus the group's closed time; an owner answers a step at 8
  with `Stepped`, as version 8 did. At 8 both `Step.carried` and `Stepped.latest` carry the backup
  clock's nanoseconds. At 9 the stamps are microseconds. Each side speaking 9 converts at the
  version-8 boundary, rounding an incoming nanosecond stamp up so its clock covers it (T-T1.12.X2).
  `Step::Lock` is version 8's and sent only at 9, as what it locks is finished by version 9's steps
  (xgroup.md, "Over the transport").
- What a version-8 peer can't say is missing, not guessed: a step a version-8 node answers adds no
  closed time to a transaction's token (xgroup.md, "Reading its own writes across groups"), so
  reading one's own writes in that node's groups holds once it speaks 9.

`VERSION_MIN`, 8, is a format value: every version-8 message keeps its shape, and nothing newer is
sent at 8. The tests: `a_version_8_client_and_a_version_9_owner_exchange_what_both_know`,
`a_version_9_client_and_a_version_8_owner_exchange_what_both_know`,
`a_version_9_greeting_agrees_on_9_and_binds_it`, `negotiated_versions_take_the_newest_both_speak`,
`t_t1_9_r30_a_participant_too_old_to_finish_is_refused_before_any_lock`,
`t_t1_9_r30_the_transports_frames_keep_their_wire_numbers`.
Falsified by: a version-8 peer refused by a version-9 one, a variant sent on a connection that
agreed on a version without it, or a greeting with its `Speaks` cut off that authenticates.

## Version 10 (2026-11-23)

`Refusal` gained `Serialization` at its end: a write refused because what it read changed before it
applied (the stale-base rule, replication.md), nothing of it written, to be asked again, as
PostgreSQL's 40001 `serialization_failure` says. Until then it was `Invalid`, which says the
caller's input was wrong; callers told the two apart by its text. Its text is unchanged ("invalid
prepared transaction: the database advanced since preparation"), so a caller matching it still
retries. A version-9 peer can't decode the variant, so an owner sends it only on a connection that
agreed on 10 or later, and as the `Invalid` it was below that (`spoken`). The `Q` connection's
frames (serve.rs, `SQL_VERSION` 3) do the same. The tests:
`an_overlapping_write_is_refused_serialization_and_a_malformed_one_invalid`,
`a_peer_before_version_10_is_sent_serialization_as_invalid`.

## The tests' budgets (2026-10-31)

None of the transport tests is about latency. Their budgets were real-clock guesses, and a loaded
disk or a stopped process overran them:
- the client's `until()`: five seconds in `transport/common.rs`, ten in `transport/watch.rs` and
  `tests/transport.rs`;
- the one-second session `wrong_capability_never_acquires_database` asked for in its `Hello`;
- `serve`'s fixed two-second greeting.

Once `compact_step_on` has sent a mutation, it answers UnknownOutcome for an answer read past its
deadline. That is correct, since the mutation's fate is then unknown, but the test took it for a
failure. At 2e7728d, stopping the test past its budget gave that failure, and so did the base
327a8f9. Measured under load, the test failed 5 times in 200 runs at loads 24 to 37, each run
inside its five seconds and past its one-second session.

Every operation of these tests now waits under one hang guard, `HANG`: 120 seconds. It is a
measured constant (CLAUDE.md section 5): twenty times the slowest whole run of any transport test,
measured over 200 runs of each of the 12 tests at loads 19 to 39 (5,856 ms, kiln plan
01a1905036ce70ebae3484dc384f8129 step 2). Each test's run bounds every operation in it. `HANG`
replaces:
- `transport/common.rs`: `until()` in `wrong_capability_never_acquires_database` (11 uses: the
  greeting's writes and reads, the Capabilities request, `compact_step_on`);
  `frame_length_is_refused_before_reading_payload` (5);
  `stale_record_cannot_authenticate_a_reused_port` (4, `connect` and `read_limit`);
  `a_stop_and_continue_leaves_a_waiting_read_whole` and `..._send_whole` (1 each). Their
  deadline need only outlast their stop, which the test makes a fraction of a second.
- `transport/watch.rs`: `until()` in `completion_before_poll_does_not_need_a_registered_waker` (2),
  `registration_overflow_is_typed_and_drop_releases_capacity` (2) and
  `sixty_four_dormant_watches_share_one_reactor_and_leave_sql_usable` (2).
  `hostile_watch_batch_lengths_are_bounded_before_element_decode` has none.
- `tests/transport.rs`: `until()` in
  `independent_processes_share_cas_and_adaptive_sql_and_abort_disconnect` (14),
  `sdk_client_keeps_ledger_and_becomes_owner_after_predecessor_exits` (12),
  `separate_memory_databases_do_not_share_sql_or_records` (7) and the child half they start,
  `transport_child` (10).

A session can't be given `HANG`. `serve` refuses a session longer than `SESSION_MAX`, 30 s, the
longest the protocol allows, and the production client asks for at most that. So the session
`wrong_capability_never_acquires_database` asked for in its `Hello` (1,000 ms) is now
`SESSION_MAX`: the protocol's own bound, five times the slowest run measured.

`HANG` also bounds the tests' own waits:
- `pair()`'s loopback connect (two seconds);
- the send test's peer read (five seconds), whose timeout only keeps a short send from hanging the
  read;
- the waits that took their deadline from `until()`: `wait_asleep` polling a thread's state, the
  watch fixture's wait for its notice, and `tests/transport.rs`'s wait for a replica to be ready.

One clock read stays as it is: `successor.replica_status(Instant::now())`, in
`sdk_client_keeps_ledger_and_becomes_owner_after_predecessor_exits`. Its deadline has already passed
when it is read, the call must refuse with Timeout, and so it answers the same at any load.

`serve` takes its greeting's budget from its caller. The owner passes `GREETING_MAX`, the two
seconds to authenticate of the resource limits above: its bound on a peer that connects and says
nothing. A test that runs `serve` itself passes `HANG`. The client gives the owner the same
`GREETING_MAX`: its bound on a listener that accepts and says nothing, such as another program on a
stale record's port. Both stay, because they are admission policy, and the tests that go through
the production client or owner meet them: `stale_record_cannot_authenticate_a_reused_port`, the
watch tests and `tests/transport.rs`. A greeting reads the owner's 34-byte record, connects over
loopback, and exchanges three frames of at most 256 bytes with an HMAC each way. In the same
measurement, the slowest whole run of the test that is little more than one greeting,
`stale_record_cannot_authenticate_a_reused_port`, took 589 ms. A process stopped for longer than
`GREETING_MAX` inside a greeting answers Timeout, as production must.

This is not "a longer timeout" in CLAUDE.md section 4's sense. No test's verdict rests on `HANG`:
each test's subject completes in milliseconds, and its assertion is about the outcome. The guard
fires only when nothing has happened for twenty times the slowest run measured, which is a hang,
not a slow disk. Its value is measured, not guessed.

Evidence: kiln plan 01a1905036ce70ebae3484dc384f8129 and its research
01a1905036d67622b5262d141422fdcf: the failure and its message at 16c8ed9 and 327a8f9, the budgets
of the 13 tests, and the measured tails. Dean and Barroso, "The Tail at Scale" (CACM 56(2), 2013):
on shared machines the slow tail, not the typical time, decides whether a budget holds.

Falsified by:
- a transport test that fails with Timeout or UnknownOutcome before its `HANG` has passed, unless
  it was stopped for longer than `GREETING_MAX` inside a greeting;
- a transport test whose outcome changes with the machine's load;
- a measured whole run past a twentieth of `HANG`. That last one calls for measuring again, not for
  a larger guard.

## Owner guards (2026-11-06)

kiln's Windows host store keeps handles that pin its database's files and directories (its
"guards") and must hold them as long as Amphora has those files open. Codex proposed the hook
(kiln-unify proposal sha256 6e2e9c80, 2026-10-30); the Amphora agent decided its rules the same day
(the decisions sent to Codex as amphora-reply-codex-keepalive-decisions-2026-10-30). Three
additive constructors carry them, and the existing ones keep their behavior:
`Database::open_guarded`, `Database::open_replicated_guarded` and
`Database::attach_replicated_guarded`, each taking a `Guards::new(key, keep)`.

- `keep` is held, never read. `key` is the host's assertion that two groups protect the same files;
  Amphora compares it and does not verify it. No token type, serialization or wire change.
- The owner lease and the guards travel together as one `Owned`, cloned wherever the lease is held:
  the `Db` (and each reader), a reader's opener, the flusher, `Shared`, the replicated actor and its
  `Machine`. The owner table still holds a `Weak` of the lease alone, so drain is decided as before:
  the last holder of the lease has dropped. `Owned` declares the lease before the guards, so the
  group outlives the lease it came with.
- The engine's handles drop before the lease and its guards in every holder. `Db` declares its owner
  after its engine fields; the opener and the flusher capture one `EngineHandles` (engine IO,
  database, then owner) as a whole. Before this, the flusher captured the lease as its own
  variable, and the Reference leaves the drop order of a closure's moved captures unspecified;
  measured at this change, rustc dropped the lease while the flusher's engine handle was still alive
  (`db::tests::the_owner_guards_drop_after_the_engine_in_every_holder` failed with one engine
  handle left, and passes with `EngineHandles`).
- A guarded open makes no directory: its database and log paths are made canonical without
  creating a parent (`existing_path`), and a missing parent is refused before the owner table, the
  endpoint or the lease is touched. The lease then makes only the database's own `.client`
  endpoint directory, as every owner's does. The steps after resolution make nothing either: a
  guarded owner's engine opens without `create_dir_all` (`open_db`, for the local writer and the
  replicated machine), and its log's lease resolves its path the same way (`exclusive_file_of`).
  A parent removed after the open resolved it is refused with the same error, never made again: a
  remade directory is another directory (the host binds its group to the parent's file identity),
  and the host's guards would protect nothing, with nobody told. A parent removed in the narrow
  window between that resolution and the engine's or the lease's open ends as an I/O error
  (`Refusal::Unavailable`), not this typed refusal; nothing is made there either. Asked by
  amphora-seam-owner, in Codex's place, on 2026-11-06.
- Under the owner table's lock, before any endpoint or lease effect, a guarded open meeting a live
  owner in this process admits it only if that owner is not a client, the file at the path now is
  the file it stored (its identity), and it holds guards of the same key. Then the incoming guards
  are dropped and the owner keeps its own group: one group per owner, never accumulated. An
  unguarded owner, other guards, or another file at the path is refused with
  `Refusal::Invalid` ("database ownership"): that owner must drain first. An unguarded open may
  reuse a guarded owner, whose group stays.
- Another process's owner is refused: its identity can't show which files it opened, so there is
  no guarded client and no guarded promotion. Guarded means this process is or becomes the
  owner. The refusal follows the failed lease attempt, the only way to learn of that owner, which
  changes nothing.
- Guarded opens are persisted: no guarded constructor takes a durability (replication.md, "Fast
  acknowledgement: the replicated class"), since no host asks for one; a guarded host that wants the
  replicated class gets it through a later additive constructor. An open or attach in this process
  that states the replicated class at a guarded owner is refused `Refusal::Invalid` ("replica
  ownership", a different live owner), never answered persisted in its place
  (`database::remote::tests::a_guarded_owner_refuses_a_replicated_open_or_attach_in_this_process`).
  Another process's attach stating it is refused the same way at the transport: the owner's
  identity names its durability (version 4), and a different one is refused, typed (replication.md,
  "Fates and durability checked"). Agreed by amphora-seam-owner, 2026-11-06.
- `attach_replicated` now checks the owner bound before it probes the lease. The probe makes the
  endpoint directory and holds the lease for an instant; at a full table attach answered "no
  owner" after those effects, and the open that followed was refused at the same bound
  (`database::remote::tests::attach_refuses_at_the_owner_bound_before_it_touches_the_lease`,
  failing before the fix). `Database::owner` already bounded first.

- The unguarded opens decide their refusals before they make a directory too (kiln goal 017b5e3d,
  reported to Codex's role as a side effect before a refusal). They resolve paths with
  `resolved_path`: the deepest existing ancestor made canonical, then the missing names below it,
  as `create_dir_all` will make them. The database's parent is made by its lease, after the owner
  table's refusals and bound; the log's by its own lease, once this process owns the database; the
  replica configuration is checked before either. `attach_replicated` makes no directory at all: no
  owner can hold a lease under a directory that doesn't exist, so it answers None. A missing
  directory named by `..` can't be resolved before it is made, and is refused.
- What remains, decided (amphora-seam-owner, in Codex's place, 2026-11-06): the refusals that need
  the database's contents come after the log's lease. `replicated::open` takes the log's lease
  first, which makes the log's parent directory and its `<log>.client` endpoint at the log path
  given. Then `Machine::open_with_owner`'s refusals (such as an unfinished restore, a baseline
  declaration whose source isn't an initial voter, a replica table without its applied marker)
  leave those behind. Then the log file is made, and the fence "state exists but its replication log is
  missing" (an empty log beside a database whose durable index isn't zero) leaves an empty log
  file as well. Each is a typed, fail-closed refusal, and nothing in the database changes. They
  aren't moved before the log's lease: opening the machine first would write its applied marker
  into the database before the log is held, an effect on the database itself, and the fence can't
  be decided earlier, since it needs the machine's durable index.

- A reopen beside a child that shares a drained lease (J1, 2026-11-07, decided with
  amphora-seam-owner). On Unix the lease is a `flock` (std's `File::try_lock`: "This function
  currently corresponds to the flock function on Unix with the LOCK_EX and LOCK_NB flags"), and a
  flock belongs to the open file description: "duplicate file descriptors (created by, for example,
  fork(2) or dup(2)) refer to the same lock", released only "when all such file descriptors have
  been closed" (flock(2)). A forked child holds copies of every descriptor (fork(2)) until its exec
  closes the close-on-exec ones (execve(2)). So when this process drains an owner while it, or a
  library in it, forks, the lease can read held for a moment after the last holder here let go:
  62.8 µs, measured on fedora during t1/guards-flake. A guarded open, a guarded attach and a restore
  answered that moment with an Invalid naming another process's owner, which tells the host not to
  retry, though no other owner existed.
  - The owner table remembers the paths it drained, read before its retain forgets them: at most
    4,096 (the owner bound), the oldest evicted first, and a path's mark cleared once this process
    takes its lease again.
  - A guarded open, a guarded attach or a restore that finds the lease held at a path this process
    drained waits for it within the caller's deadline, with the owner table unlocked between
    probes; at the deadline it answers `Refusal::Timeout`. Its pauses are learned, not picked
    (2026-11-11; Ada: "We don't tolerate arbitrary constants, learned and stat derived values
    only"; they were 50 µs doubling to 1 ms). The first pause is the probe's own measured cost,
    so probing takes no more time than waiting. Each next pause is twice the last, at least the
    probe's cost. It is capped at the larger of the window this process last waited out before
    taking a drained lease (`DRAIN_WINDOW`, from the first probe to the lease taken: one last sample,
    which bounds how late a probe is, never how long a wait lasts) and this wait so far. So a lease
    let go within the learned window is taken within it, and a drain that outlasts the window (a
    lease held to the deadline, K1) goes on doubling instead of probing at the window's pace, about
    16,000 times a second at 62.8 µs (amphora-seam-owner-2,
    amphora-owner-t1-constants-code-20261111). Before any window is learned the pauses double to the
    deadline, so a lease is taken within twice the wait. `database::drain_pause` tests the
    pure core (`next_pause`). Falsified by a reopen after a drain answering later than one learned
    window past the lease's release, or probing for longer than it waits. A path with no drain of this
    process behind it keeps the at-once refusal (`remote_owner()`, the restore's "another process
    owns the destination"). An unguarded open already waits: it attaches as a client, the drained
    owner answers nothing, and the open retries the client's `Unavailable` to its deadline.
  - Why this is not the retry CLAUDE.md section 4 forbids: it is the wait on the fact the call needs
    (the lease free), at the cause found (a sharer outside this process that lets go at its exec),
    with the bound section 2 asks, and it fixes no test by waiting longer. The cost: a path this
    process drained that another process then owns for good is answered `Timeout` at the deadline
    rather than the at-once Invalid, until this process owns it again or the mark is evicted. Both
    are typed; Timeout says try again. Refusal's Timeout carries no text (a wire enum, changed
    append-only), so at a marked path's deadline the owner logs the reason, event
    `records.lease_held_since_drain` on `amphora.records` (warn; agreed by Kiln Metrics,
    `amphora-replicated-line/events/v1`): "the lease is still held since this process drained
    it", which an operator tells from a slow open.
  - Windows has no fork: a child gets only the handles it is given, so the window is Unix's
    (Linux and macOS), and the wait is the same code on every platform.
  - Tests: `a_child_sharing_the_lease_holds_it_past_this_processs_drain` (the rig),
    `a_guarded_reopen_beside_a_child_sharing_the_drained_lease_times_out`,
    `a_guarded_attach_beside_a_child_sharing_the_drained_lease_times_out`,
    `a_restore_beside_a_child_sharing_the_drained_lease_times_out` and
    `a_guarded_reopen_owns_the_database_once_the_sharing_child_lets_go`, red at main f99b758.
  - Falsified by: an Invalid answered at a path this process drained while no other owner exists;
    a wait past the caller's deadline; a wait at a path this process never drained; more than 4,096
    marks held.

Evidence: `database::guards::tests` with a Drop witness: sixteen same-key opens and an unguarded
one keep one group, which outlives every `Database` clone and drops with the lease free (after the
writer, readers and flusher drained); "free" is the owner table's own drain, no holder of the lease
left in this process, not the lock: a child this process forks shares the lock's open file
description until its exec, so a probe of the lock found it held for microseconds while other
tests spawned children, and the witness failed 2 times in 273 loaded runs (2026-11-07, fedora;
`a_child_sharing_the_lease_does_not_hold_the_group`, red against the lock probe); other guards, an unguarded owner, another file at the path
and another process's owner are each refused with the incoming group dropped at once and the live
owner untouched; a missing parent is refused with nothing made, and a parent removed after resolution
is refused by each later step and not made again; the replicated owner is reused and
attached by the same key. Removing the admission, or resolving guarded paths with
`canonical_path`, fails these tests. They show Amphora's side on each platform; whether the host's
handles protect a Windows path is the host's own test.

Sources: the Rust Reference, "Destructors"
(doc.rust-lang.org/reference/destructors.html, read 2026-11-06): "The fields of a struct are
dropped in declaration order", and "The variables that a closure captures by move are dropped in
an unspecified order". Rust's `File::try_lock` (cited above) for the lease's release on the last
handle's close.

Falsified by: a group dropped while a reader, opener, flusher, actor or machine of its owner still
holds an engine handle; two groups alive for one owner after a same-key reopen; a guarded open
that makes a parent directory (at resolution or after it), reuses an unguarded, differently keyed or remote owner, or reuses
an owner whose stored file identity differs from the file at the path; or attach making the
endpoint directory at a full owner table, or an unguarded open refused by the owner table, its
bound or its configuration having made a directory
(`database::remote::tests::refused_replicated_opens_leave_no_directory_they_made`,
`opens_refused_at_the_owner_bound_make_no_directory`).

## Decision (lane constants): one descriptor budget (2026-11-21)

Ada's rule: no picked numbers, only learned or derived ones. `CONNECTION_MAX` (32 a database, 2026-10-29) was
picked: its own paragraph called it admission policy. It bounded three things: the listener's backlog, the
connections an owner served at once, and, through `READERS`, the local readers out on one replica
(replication.md, "A replica's local readers bounded by derivation"). amphora-seam-owner-2's ruling (d), 2026-11-11:
derive it from the process's descriptor limit, as one budget shared by what holds descriptors, never the whole limit
for each, and with a named Windows source.

**What a connection costs.** One descriptor, its accepted socket, held from `accept` until its worker ends. The
system refuses a descriptor past the process's limit (`EMFILE`), so that limit is what bounds connections.

**The decision.**
1. **One budget a process, the system's as it stands** (`crates/amphora/src/descriptors.rs`): the system's limit
   less the descriptors open outside the budget. Nothing is read once and kept (Ada, 2026-11-21: a value measured
   once and frozen is a picked constant). The limit is read again at every charge, so a host that raises it while
   it runs is followed. What is open outside (the host's own, the owners' files) is counted again whenever the
   limit moves, whenever a charge would be refused (the host may have let some go), and whenever the system refuses
   a descriptor the budget allowed (`EMFILE` at `accept`). The limit is the soft `RLIMIT_NOFILE` (POSIX
   `getrlimit`). With none set (`RLIM_INFINITY`) nothing here bounds them, and the kernel's own ceiling (Linux's
   `fs.nr_open`) refuses past it. Those open are counted from `/dev/fd` (Linux's link to `/proc/self/fd`; macOS's
   fdesc). A charge between a count's two readings leaves that count off by it once: one too many refuses a charge
   until the next count (which the refusal itself makes), one too few meets the system's own refusal (item 4). On
   Windows a process sets no descriptor limit: its handle table holds at most 2^24 handles, a structural limit of
   the system, and Winsock's sockets come out of the same table (Russinovich; `WSADATA::iMaxSockets` is ignored
   since Winsock 2). Windows says a process's open handles only through `GetProcessHandleCount`, which needs
   `unsafe` (CLAUDE.md section 12), so none are counted there.
2. **A connection charges its socket.** Each accepted connection takes one descriptor from the budget before its
   worker starts, and gives it back when the worker ends. Past the budget the connection is closed unserved, and the
   client gets a typed refusal before submission, as before. No count of connections is left.
3. **The backlog is the system's own most.** `listen` is given `i32::MAX`: Linux truncates it to
   `net.core.somaxconn` (listen(2)), XNU to `kern.ipc.somaxconn` (`solisten`), and Windows reads `SOMAXCONN`
   (0x7fffffff) as "a maximum reasonable value" (Microsoft, listen function). A queued connection holds no
   descriptor of this process until it is accepted.
4. **The system refusing first is not fatal.** Descriptors no budget charges (the host's own, the owners' files) come
   out of the same limit, so `accept` can meet `EMFILE`/`ENFILE` (`WSAEMFILE` on Windows) before the budget is
   spent. The listener then counts again what is open outside the budget, leaves the connection queued and goes
   on; before this change it stopped for good.
5. **The local readers follow.** A replica's readers out at once are at most the clients that can be attached at
   once (replication.md's reasoning, unchanged): now the budget's bound as it stands at each read. Readers hold no
   descriptor (`a_local_reader_opens_no_file`), so they charge none; their pages stay charged to the page budget,
   which is what bounds them in practice.
6. **An owner charges its descriptor; no count of owners** (`OWNERS_MAX`, 4,096 since before 2026-11-11, was
   picked). Each owner takes one descriptor from the budget at admission, before any probe makes a directory or
   takes a lease: a local or replicated owner's is its lease, held beside it (`Owned`) and given back once the lease
   is closed; a client's is the socket its calls open, held by its `Backend` while it lives. The other leases
   (`Machine::open`, bootstrap's inspections) charge theirs the same way. Past the budget an open or attach is
   refused `TooLarge` and makes nothing. The owner table's entries each hold such a charge while alive, and the dead
   are forgotten before each admission, so the budget bounds the table. The drained paths kept (`DRAINED`, J1) are
   at most the budget's bound as it stands, oldest evicted: each names a lease this process held, one descriptor of
   it. An evicted mark makes an open that still finds the lease held refuse it as another process's, typed, rather
   than wait.

**Tests.**
- `descriptors::tests::a_budget_holds_its_bound_and_takes_back_what_is_given` (T-FD.1.1).
- `descriptors::tests::the_process_budget_follows_the_systems_limit` (T-FD.1.2): the soft limit raised by one while
  the process runs, and put back; the budget's limit follows both at its next charge.
- `transport::common::tests::an_owners_connections_are_bounded_by_the_descriptor_budget` (T-FD.1.3): under a
  budget of 1, a second client is turned away while the first is served, and served once the first's descriptor is
  given back. With the old count of 32, the second was served.
- `replicated::tests::local_readers_are_bounded_by_the_transport_not_four` (T-T1.10.L8): under a budget of 6, six
  readers out and the seventh waits to its deadline; the bound moves with the budget.
- `database::remote::tests::an_owner_charges_its_lease_to_the_descriptor_budget` (T-FD.1.4): under a budget of 1, a
  second database's open is refused while the first is open, and served once the first has drained.
- `database::remote::tests::opens_refused_at_the_descriptor_budget_make_no_directory` and
  `attach_refuses_at_the_descriptor_budget_before_it_touches_the_lease` (U05): with the budget spent, open,
  open_replicated and attach_replicated are refused `TooLarge` and make no directory, endpoint or lease.

**What is left (not in this change).**
- An owner's other descriptors (the endpoint's lock and listener, the engine's database and log files, a replica's
  log) are charged by no kind yet: they come out of the same limit as descriptors open outside the budget, counted
  again at the moments item 1 names. Charging each where it is held, stated by kind and held to the count by a
  `/dev/fd` test, is the next step.
- Memory: a connection's frames are bounded each (33 MiB), but their sum across connections is charged to no memory
  budget. Before, 32 connections a database bounded it loosely (and not across databases). It waits for the
  process memory budget (the same one `SNAPSHOT_BYTES` waits for).
- Threads: each connection is a thread. A thread the system refuses is logged (`worker_start_failed`) and its
  connection closed unserved.
- A host that wants more connections raises its soft limit (as Go's runtime raises it to the hard limit at start);
  Amphora, a library in its host's process, doesn't change a process-wide limit.

**Sources.** POSIX `getrlimit` (pubs.opengroup.org, getrlimit, `RLIMIT_NOFILE`); Linux listen(2) and proc(5)
(`/proc/sys/net/core/somaxconn`, `/proc/sys/fs/nr_open`); XNU `bsd/kern/uipc_socket.c` (`solisten`); Microsoft,
"listen function (winsock2.h)" and "WSAStartup function" (`iMaxSockets`); Microsoft, "Windows Sockets Error Codes"
(`WSAEMFILE`, 10024); Mark Russinovich, "Pushing the Limits of Windows: Handles" (Microsoft, 2009).

**Falsified by:** a connection served while the budget's descriptors are all held; a connection refused while the
budget has room and the system allows it; the listener stopping on `EMFILE`; more readers out on one replica than the
budget's bound.
