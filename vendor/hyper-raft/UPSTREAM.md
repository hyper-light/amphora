# hyper-raft's shared crates, vendored

The crates Focal's bus is built on (`vendor/focal-bus`), at hyper-raft's `main`.

- **Upstream:** https://github.com/hyper-light/hyper-raft, commit
  `4d72e19b9533851e19602eee57053a5a7337b2cd` (2026-10-28), MIT (`LICENSE`): hyper-raft's `main` since
  its `main` and `fate-commit` lines met (the merge `7d5e536`). Focal's exact `f8052a5` base
  originally used `25cf889`; its recorded compatibility patch adapts the bus to this shared pin.
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

| `crates/hyper-raft/src/{lib,raft,tests}.rs` | `Raft::commit_stats`: a leader counts the entries it commits by the quorum that committed each first, `CommitStats { classic, held }`, counted where `Raft::commit` takes the first of the two (R-8, `docs/raft.md` §3.7); its test `a_leader_counts_each_commit_by_the_quorum_that_made_it_first` | Amphora's replicated class exports its commits by quorum (`amphora_replica_commits_total{quorum}`, docs/design/replication.md "Metrics", kiln goal 8f0f29ac), and only the core knows which quorum moved its commit; no wire or behavior change | `0006-r-8-a-leader-counts-its-commits-by-quorum.patch` |

The first change is Quiver's and kiln's policy, not hyper-raft's, so it isn't offered upstream: hyper-durable
calls `RamStore` slates' case (each write complete as it's submitted), and gating it in hyper-raft
would reach slates. Focal's agent agrees (2026-10-22), and notes that Focal names `RamStore` only in a
test module (`focal-consensus`'s `shell_tests.rs`): sharing this copy, it would ask for `test-store`
as a dev-dependency, as kiln's sync tests do.

The former IOCP patch `0002` is superseded by upstream at `53a28a0`, with its regressions, and is dropped.
The Darwin FIFO patch `0003` remains: upstream kqueue code is unchanged at this pin.

`0006` is upstream's to take. It is offered as one commit on upstream's main: `d5d5f8f`, on `0856e05`, branch
`upstream/commit-stats-main` in a clone kept at `~/.cache/hyper-raft-class-metrics`, with its push remote disabled.
- Its test was red before the count. hyper-raft's `scripts/gates.sh` (contracts, fmt, clippy with all features,
  and every test: 2,904 passed, 0 failed, 98 ignored) exited 0 on an M4 Max (adas-mac-studio.local-2,
  2026-11-06); the other five CI targets haven't run it.
- Upstream main's seven commits past this pin don't touch the files it changes, so the vendored patch is the same
  change.
- Pushing it is Ada's word. Once upstream takes it, a re-vendor drops the patch.

## Taking a newer upstream

`python3 scripts/vendor.py hyper-raft <commit>` takes these crates at that commit and re-applies the
changes above by three-way merge; coordinate the consumer compatibility patch and the identical pin/patch set in every product.
