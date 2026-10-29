# Embedded local owner transport (CP-T1.5, U05/U06)

The canonical database owns one engine, controller and durability pipeline. A second process
uses a product-owned local client; it never opens another engine. The embedding Database must
select owner or client during startup and retain the listener in shared ownership. The listener
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

Windows binds SO_EXCLUSIVEADDRUSE before bind through socket2's safe API; Linux/macOS do not
request reuse of an already bound listening endpoint. The exact upstream socket2 revision is
pinned because the safe Windows exclusivity setter merged after its latest published release.
There are no product unsafe blocks or production subprocesses.

Both peers authenticate the owner before revealing a client proof: random client/server nonces
and direction-separated HMAC-SHA256 proofs bind protocol version, nonces and requested lifetime.
The capability never crosses the socket. Thus a stale record whose port was reused by another
process cannot authenticate that process or expose the capability. Windows exclusive binding
prevents another local socket from intercepting or relaying traffic after the handshake. OS-local
stream isolation protects subsequent SQL/Records frames; this is not a network transport.

The wire is a four-byte big-endian length followed by a postcard envelope. Variant order is
append-only. Frames are limited to RESULT_MAX plus 1 MiB of serialization overhead. Serialization
grows a buffer only up to that fixed cap; small control messages keep small allocations. Reads and writes reset their kernel timeout to the remaining
absolute deadline after every partial transfer. All platforms use TcpStream::connect_timeout. Backlog saturation refuses before submission. No retry loop replays writes or closures.

Resource limits are admission policy, not measured tuning claims: 32 connections, at most eight
Records waits, 30 seconds total per authenticated connection, two seconds to authenticate,
4096 commands per adaptive session, 256 bytes per authentication frame, and at most 33 MiB per
data frame. Collection decoders bound statement/parameter/transaction arrays before allocation
and charge SQL result collection overhead as well as value payload. Overload closes an unadmitted
connection; the client returns a typed connection refusal before submission. The persistent
accept loop checks shutdown every 10 ms (bounded idle shutdown latency); each admitted worker
releases its permit on every exit. Waits use existing Namespace waiting outside the SQL writer;
they cannot consume all connection slots. Frame allocation caps bound per-process transport memory at 32 connections times bounded wire
and decoded values. Authentication frames have a separate 256-byte cap.

Adaptive SQL is executed inside existing Database.call. The client closure runs once. Intermediate
query responses are provisional; Commit ends the closure and the final Done waits until the
real durable callback. Socket loss, deadline expiry, exhausted command budget or an explicit
Abort before Commit causes the closure to fail and the existing pipeline to roll back. Loss of
the final response after Commit is an unknown outcome; callers must not replay FnOnce. Records
CAS, SQL batches and migrations route directly to existing Database/Namespace methods.

Falsification: a second independent engine acquiring the same canonical database; a successful
response before durable publication; a disconnected adaptive session leaving rows; one clone's
drop stopping another; an unauthenticated client reaching SQL; a Records wait preventing a SQL
client's admission; or any attacker-controlled allocation beyond the frame/API budgets.
`tests/transport.rs` exercises independent-process ownership, CAS, adaptive SQL, application rollback
and disconnect rollback on both platform shells. Endpoint tests also check private record
reading under the separate lock and competing owner refusal. Builds and behavior tests must run
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
