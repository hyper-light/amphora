# hyper-raft's shared crates, vendored

The crates Focal's bus is built on (`vendor/focal-bus`), at hyper-raft's `main`.

- **Upstream:** https://github.com/hyper-light/hyper-raft, commit
  `d5d5f8f66368bb8a08822518dd45d257e95e2dbf` (2026-11-06), MIT (`LICENSE`): hyper-raft's `main` with the
  leader's commit count by quorum (R-8, `Raft::commit_stats`; Amphora's replicated class's metrics, kiln goal
  8f0f29ac), which was Amphora's patch 0006 until upstream took it. The pin before was `4d72e19` (2026-10-28),
  hyper-raft's `main` since its `main` and `fate-commit` lines met (the merge `7d5e536`). Focal's exact
  `f8052a5` base originally used `25cf889`; its recorded compatibility patch adapts the bus to this shared pin.
- **What is here:** upstream's root manifest and the fifteen crates, whole, at upstream's paths:
  `hyper-block`, `hyper-datagram`, `hyper-durable`, `hyper-liveness`, `hyper-log`, `hyper-measure`,
  `hyper-quic`, `hyper-raft`, `hyper-rt`, `hyper-rt-transport`, `hyper-seal`, `hyper-sim`,
  `hyper-timing`, `hyper-tls`, `hyper-transport`.
- **How it's built:** Quiver's root `Cargo.toml` points the names Focal's bus takes them by at these
  folders (`[patch.crates-io]`); kiln, vendoring Quiver, holds the same revision with the same
  changes, its own crates built on it too.

## Changes from upstream

Each change is recorded in `vendor/patches/hyper-raft/`, applied in this order on upstream's tree.

| File | Change | Why | Recorded as |
|---|---|---|---|
| `crates/hyper-durable/src/lib.rs`, `crates/hyper-durable/Cargo.toml` | `RamStore` compiled only for the crate's own tests or under the `test-store` feature, which a dev-dependency asks for (the crate's own tests ask through one on itself) | the write-ahead log is mandatory in Quiver and kiln: `GroupStore` on a real device, `RamStore` test-only, and a build that names it outside a test fails (kiln's CP-T2.1, T-T2.1.2; Quiver's `crates/quiver/tests/ram_store.rs`) | `0001-ramstore-for-tests-alone-the-write-ahead-log-mandatory.patch` |

| `crates/hyper-rt/src/kqueue.rs` | macOS FIFO readiness shares the existing reactor wait through Rustix unlimited select over its kqueue descriptor and bounded FIFO registrations; critical timers, kicks, one-shot ownership and typed invalid-FD refusals remain intact | CP-M1: native Darwin probes show quiet writer-close EOF is readable but neither kqueue nor poll signals it; native r57 validates backend fixtures, original two-second coalition/recovery, terminal and process lifecycle tests without threads or timeout polling | `0003-cp-m1-darwin-named-fifo-eof-in-the-existing-reactor.patch` |

| `crates/hyper-transport/src/{lib,endpoint}.rs`, `tests/exchange.rs` | Frames retain their actual connection epoch; an exporter lookup accepts exactly that peer/epoch, sharing the existing live-connection lookup | Authenticate delayed control capabilities and readiness against their true TLS origin without selecting a replacement connection, changing transport wire bytes, or adding another connection owner | `0004-cp-t2-2-frame-connection-provenance-and-exact-tls-exporter.patch` |

| `crates/hyper-durable/src/replica.rs` | `Replica::suspect_held` and `Replica::trust_held`: the owner's node-level detector's verdicts for the held count alone (core step R-8, `RawNode::suspect_held`, `trust_held`), additive | Amphora elects on ticks, where `Replica::suspect`, `trust` and `restarted` are refused once they've taken the held part; its replicated class needs a suspected member left out of held quorums without its elections taking a detector's word (Amphora's docs/design/replication.md, "The suspicion feed") | `0005-cp-t1-8-the-held-count-fed-on-ticks.patch` |

| `crates/hyper-raft/src/{raft,progress,fast,read,lib}.rs`, `crates/hyper-durable/src/replica.rs` | `Raft::grow_members` raises the most members a configuration may name (the tracker's, the fast votes' and the pending reads' bound with it), never lowers it; `Config::stated_members` (`StatedMembers`, `None` by default) is the owner's reader of a bound its entries state, which the core folds in as it takes entries, at open as after, before the configurations they change take effect; `Replica::grow_members` passes the raise through, additive | hyper-raft stops a member handed a configuration past the members it was opened for, so a group that grows needs its bound to grow at every member: Amphora states the bound once in its log and raises it ahead of a change that passes it (Amphora's docs/design/replication.md, "The member bound stated once per group"; QB.1, lane constants-21) | `0006-qb-1-the-member-bound-stated-in-the-owners-log.patch` |

| `crates/hyper-durable/src/replica.rs` | `Replica::quiet`: the owner's quiet period in force, read back, additive | Amphora sets its quiet period from its learned election timeout as its pace moves (`Replica::set_quiet`; Amphora's docs/design/replication.md, QB.1, lane constants-21), and its test reads it back | `0007-qb-1-the-owners-quiet-period-read-back.patch` |
| `crates/hyper-durable/src/{replica,held,machine}.rs`, `crates/hyper-durable/tests/shell.rs` | `Replica::open` takes the image's bytes from `StateMachine::image_bytes` alone; a log compacted before the restart has its snapshot prepared by the first `Replica::drive` after the core asked for one (`Held::snapshot` notes the ask while none is prepared) | A member that opened on a compacted log serialized its whole state machine into memory at open only to weigh the log against it (Amphora's AUDIT §7.6, perf-H3): a 10 GiB database took 10 GiB of memory and its full read to open | `0008-audit-7-6-a-compacted-log-s-snapshot-prepared-when-first.patch` |
| `crates/hyper-raft/src/{node,raft,tests}.rs`, `crates/hyper-durable/src/replica.rs`, `crates/hyper-raft/tests/support/{lagged,mod}.rs` | `RawNode::light_now` asks the round for the reads waiting (`Raft::ask_reads`) and gives the reads confirmed since the last `Ready` (`LightReady::read_states`), which `Replica::take_light` takes; under `Config::answer_beats_now` (off by default, as raft-rs's rule), a member whose term and vote are durable (the term and vote durable now kept by `Raft`, `durable_vote`) answers a heartbeat at once, beside the held answers, its position the durable end of its log (none where that end's term reads 0, a snapshot restored and not yet durable), its commit the durable one and its incarnation as every answer states it (`Raft::answer_beat_now`); the core's tests take the held answers a `Ready` gives, keep the default rule and hold the new one under the option; the tests' `Settings::answer_beats_now` sets it, the lagged harness turns it on, and its oracle holds a heartbeat answer's position of its own term to the disk | A write's ReadIndex barrier waited on two log flushes: the leader's read round and its confirmation left only with a `Ready`, which its write out held back at Amphora's depth 1, and a follower's heartbeat answer only with no write out. Neither change promises more: the reads' rule is ReadIndex's, and a heartbeat answer states nothing its disk doesn't hold, so its leader never counts a position past that member's durable log. The early answer is an owner's option, off by default, so the core in raft-rs's settings agrees with raft-rs (`tests/differential.rs`); Amphora's replicated owner turns it on (Amphora's lanes V3 to V5, AC-T1.8.3; bench/records/2026-11-23-adacachy2-perf-commits.md) | `0009-v3-reads-confirmed-and-heartbeats-answered-without-a-write.patch` |
| `crates/hyper-durable/src/replica.rs` | `Replica::hold_apply` and `Replica::apply_held`: the owner's hold on what is committed, one more reason for the commit fence's range to wait (`apply_range` opens the fence while held, `release_fence` waits while held, and once lifted the next drive walks the range as one past a drive's page, the commit fence's rule kept), additive | Amphora runs a caller's session on a connection of its own, lent the database's one write lock, while its owner goes on ticking, voting and appending, so a session longer than an election timeout no longer costs the group its leader (Amphora's docs/design/replication.md, "Decision (lane P6)", CP-T1.13) | `0010-cp-t1-13-the-owner-s-apply-hold-beside-the-commit-fence.patch` |

The first change is Quiver's and kiln's policy, not hyper-raft's, so it isn't offered upstream: hyper-durable
calls `RamStore` slates' case (each write complete as it's submitted), and gating it in hyper-raft
would reach slates. Focal's agent agrees (2026-10-22), and notes that Focal names `RamStore` only in a
test module (`focal-consensus`'s `shell_tests.rs`): sharing this copy, it would ask for `test-store`
as a dev-dependency, as kiln's sync tests do.

The former IOCP patch `0002` is superseded by upstream at `53a28a0`, with its regressions, and is dropped.
The Darwin FIFO patch `0003` remains: upstream kqueue code is unchanged at this pin.

## The two QB.1 changes reach kiln's and Quiver's copies too

Changes `0006` (the member bound) and `0007` (the quiet period read back) are new in Amphora's
series and in no other product's yet. Ada wants one hyper-raft, so both must reach kiln's copy
(`vendor/hyper-raft` in hyper-light/kiln) and, through it, Quiver's: kiln vendors Quiver without its
`vendor/hyper-raft` (kiln's MANIFEST `exclude`), and Quiver's pin moves with kiln's (kiln's
`vendor/patches/quiver/0002`). Which series they live in, and whether they go upstream instead, is
Ada's call. The series differ (lane constants-22, read from kiln's refs on 2026-11-22):

| Amphora (pin `d5d5f8f6`) | kiln `main` (pin `e0613f52`) | kiln `land/unify`, `route/landed` (pin `fb74b595`, 2026-11-13) |
|---|---|---|
| `0001` RamStore for tests alone | `0001`, the same | `0001`, the same |
| (dropped: upstream `53a28a0` has it) | `0002` hyper-rt from kiln's own runtime (`kiln-rt`) | `0002`, the same |
| - | `0003` driver polls a turn's received events before its sends | `0003`, the same |
| - | `0004` hyper-tls: a connection's largest record set as it goes | `0004`, the same |
| - | `0005` hyper-transport says which suite and group a connection used | `0005`, the same |
| `0003` CP-M1 Darwin FIFO EOF | not on `main` | `0006` |
| `0005` CP-T1.8 the held count fed on ticks | not on `main` | `0007` |
| `0004` CP-T2.2 frame provenance and exact TLS exporter | not on `main` | `0008` |
| `0006` QB.1 the member bound | none: would be `0009` | none: would be `0009` |
| `0007` QB.1 the quiet period read back | none: would be `0010` | none: would be `0010` |

Both QB.1 changes touch `crates/hyper-durable/src/replica.rs` beside CP-T1.8's `suspect_held` and
`trust_held`, so in kiln they go after CP-T1.8 (its `0007` on the landing branches; on `main`, after
CP-T1.8 lands there). `0006` also changes `crates/hyper-raft/src/{raft,progress,fast,read,lib}.rs`,
which none of kiln's `0002`-`0005` touch. Both are additive (new methods; `Config::stated_members`
is `None` by default), so kiln's and Quiver's behaviour is unchanged until they use them. The pins
differ (`d5d5f8f6`, `e0613f52`, `fb74b595`): `scripts/vendor.py hyper-raft <commit>` re-applies each
series by three-way merge, and kiln's `scripts/vendor.py check` then holds the copy to its record.

## Taking a newer upstream

`python3 scripts/vendor.py hyper-raft <commit>` takes these crates at that commit and re-applies the
changes above by three-way merge; coordinate the consumer compatibility patch and the identical pin/patch set in every product.
