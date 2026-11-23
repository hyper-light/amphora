# Lane P5 (serve): large replicated writes answer UnknownOutcome (2026-11-23, adacachy2)

Goal: Amphora's server answers at any size, and every write commits or is refused with a typed
reason, never "outcome unknown" when the outcome is known.

## What was measured

Probes on branch `perf/serve-p5` (crates/amphora/tests/serve.rs, `p5_probe_*`), debug test build,
three `amphora serve` members in one process over loopback, class `persisted`, `deadline_ms = 5000`.
Run through kiln `run`, `on` adacachy2 (00aad8da...), needs `linux/amd64,cpus=8,mem=24g`:

    P5_SIZES=16,12 ~/.cache/amphora-perf/serve-p5-scripts/run.sh <sha> probe

One write of N rows of 1 MiB each, on a fresh cluster per size, members' status before and after:

| Write | Answer | Time | Term before -> after | Leader before -> after |
|---|---|---|---|---|
| 1 MiB | Ok | 104 ms | 1 -> 1 | same |
| 2 MiB | Ok | 186 ms | 1 -> 1 | same |
| 4 MiB | Ok | 405 ms | 1 -> 1 | same |
| 8 MiB | Ok | 820 ms | 1 -> 1 | same |
| 8 MiB (probe 1, other run) | UnknownOutcome | 837 ms | not read | not read |
| 12 MiB | UnknownOutcome | 776 ms | 1 -> 2 | 3 -> 1 |
| 16 MiB | UnknownOutcome | 1114 ms | 1 -> 2 | 3 -> 2 |

A write's time grows about 100 ms a MiB (debug build). Every UnknownOutcome came with an
election: the followers elected another leader while the writing leader ran the write's session.

40 MiB answered by a write (`p5_probe_replicated_write_answering_40_mib`): 40 rows written 4 MiB
at a time (all Ok, one election among them, term 2 at the end), then `INSERT` plus
`SELECT id, v FROM big` in one write: Ok, 41 rows answered whole, 2.59 s, term and leader unchanged.

## Cause

The caller's SQL session runs on the replica owner's thread, inside `prepare` ->
`Machine::prepare_db` -> `Db::prepare_traced` (crates/amphora/src/database/replicated.rs,
`fn prepare`; crates/amphora/src/replication.rs, `prepare_db`). While it runs, the owner neither
ticks nor sends heartbeats. A session longer than the followers' election timeout lets a follower
campaign and win (design: followers keep their own election timeout, bound by AC-T1.8.4's 1000 ms
failover; docs/design/replication.md, "The rule"). The old leader proposes the entry when the
session ends, then steps down on the higher term, and the pending write answers UnknownOutcome
(`expire`, `Phase::Proposed` with a changed term). Once proposed, that answer is honest: the
proposer can't tell whether a follower took the entry before voting.

Not the cause:

- `COMMAND_BYTES` (32 MiB, `api::REQUEST_MAX`): 12 MiB fails and 8 MiB passes, both far under it.
- The answer's path: a write's answer (P2's held frames) returns in process through the owner's
  response channel and never rides the log. 41 rows of 1 MiB came back whole from a replicated write.

Failure 2 (write plus 40 MiB select) is the same cause: whether it fails depends on the session's
time against the followers' election timeout, which the owners learn and which moved during the
run. It passed at 2.59 s once the cluster had settled in term 2.

## Fix (not landed; a decision)

The leader's heartbeats must not wait on a session. Options in the lane report.
