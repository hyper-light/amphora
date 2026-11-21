# Handoff: amphora-replica-log (2026-11-09), goals 7582f00a and aa81fbbf

## Branch
- `t1/replica-log` in ~/Projects/amphora-t1-replica-log (local, not pushed, not merged).
- Head `530091b4abb3057a717afe9eb726f0042aca49d2`, tree `4d5b6fbb5ab872fae856d2a0e1f5cce6d6422a8b`. It merges the
  batch `t1/integration-1109` d35a3f8 (class-and-readers, the WAL's small commits, PL.1, PL.3), as you asked.
  - `b33094f`: the failing tests first. Red on main's behavior on local-2, recorded in its message.
  - `0e8a610`: the code, the design, AC/T-T1.8.D1 and D2, and the metric names. Rebased onto class-and-readers 8d4c05b;
    the restart's hooks sit on its Account.
  - `739a903`: the merge of d35a3f8. Clean; nothing mapped by hand.
  - `e5a6444`: the test owed for class-and-readers. `a_promoted_joiner_counts_in_a_held_quorum`, T-T1.8.C6
    (amphora-owner-t1-class-and-readers-8d4c05b-20261109).
  - `530091b`: a test-only comment where the pump's durable hold is defined: it holds answers in transit, not the
    member's disk (the seam owner's answer to the finding below). Tests ran at e5a6444, whose code this equals.
  - Pre-rebase backup: local branch `t1/replica-log-dce4aaa`. Delete it once the batch lands.

## What it does
- Every class flushes its database's log on the owner's flushing thread; persisted applies no longer flush inline
  (aa81fbbf). Safe because persisted entries act at start: hyper-durable applies (and answers) only on this member's
  durable raft log, and a restart replays past `durable()`.
- After a flush, the flushing thread runs a passive checkpoint on its own connection, capped at the frame the flush
  made durable. The owner holds local reads back past 2 x LOG_FRAMES (131,072), or past the bound with little left,
  and restarts the log only once a pass left at most a 64th of the bound, with no read of the replica out. Busy is
  a skip; other failures fence. Disk: about 1.08 GB of database log per replica, per group (no disk budget exists).
- Metrics, as Kiln Metrics agreed (amphora-replica-log/names/v1): `amphora_replica_checkpoint_seconds`,
  `amphora_replica_database_log_restarts_total`, `amphora_replica_database_log_bytes`.

## Tested (adas-mac-studio.local-2, macOS aarch64, this tree, 2026-11-09 10:17 PST, load 9.07)
- fmt passes.
- amphora lib tests `database::`, `replication::`, `meter::` and `db::`: 145 passed, 2 ignored.
- tests replication, kiln_seam, application, transport and store pass.
- clippy -p amphora --all-targets -D warnings and scripts/check-production.sh pass.
- The timing-sensitive log, local-read, held-flush and joiner tests repeated 10 times: none failed.
- Before the rebase (dce4aaa): the 12-seed power-cut nemesis in both classes passed. Persisted: 92 writes
  acknowledged, 36 cuts, 16 of them of a leader holding writes unflushed, none lost.
Not run, the batch's: the seven gates, Linux and Windows, the 10,000-seed nemesis, the term-churn measurement.

## Left for the batch
1. The seven gates on the merged tree (`bash scripts/gates.sh`).
2. aa81fbbf's before/after term-churn measurement under load. That goal's clause "terms moved under the same load
   measured before and after and recorded" is not met by me: never run (my attempts waited on amphora-wal-redo's
   heavy lock and were stopped on your word; none took the lock). Script, in the carry repo
   ~/.cache/claude-scratch/replica-log-carry (git, commit c25bcdc): `stalls-ab.sh <bundle> <before sha> <after sha>
   <out>`. It waits for and takes ~/.cache/amphora-heavy.lock (up to 4 h), builds both trees, makes 1f7adcb's
   Studio-3 load (six looping amphora suites, a 1 GiB write-and-sync loop, spinners holding load1 in 25-55), and runs
   `production_owner_stalls_measured_on_this_host` for 1,800 s per arm, back to back. On adas-mac-studio-3, which has
   1f7adcb in ~/.cache/t1-disk-stalls-falsifier-src:
   ```
   # here: a bundle of both trees, relative to what studio-3 has
   git -C ~/Projects/amphora-t1-replica-log bundle create ~/.cache/claude-scratch/replica-log-carry/studio3.bundle \
       <AFTER sha> db991a0 ^1f7adcb
   # then a kiln background run on adas-mac-studio-3.local, cwd ~/.cache/claude-scratch/replica-log-carry:
   OUT=$HOME/.cache/t1-replica-log-stalls-$(date -u +%Y%m%dT%H%M%SZ); \
   bash stalls-ab.sh "$PWD/studio3.bundle" db991a05d1769648d1de0390ac80b04c979d150e <AFTER sha> "$OUT"; \
   cat "$OUT/run.log"; for a in before after; do echo "== $a"; \
   grep -E "^stalls:|step-downs|owner step gap|owner tick gap|write submit" "$OUT/$a.log"; done
   ```
   AFTER: dce4aaa738be234aa1f39395f34db72f1ef0607b isolates this change against main. The merged batch head
   measures the batch. Record "step-downs / elections / changes" per member, and the step gaps, in
   docs/research/owner-stalls.md as a stall count under the stated load, not a section 7 benchmark (seam owner's
   point A2). The before is db991a0; 1f7adcb's run saw terms move 169-171 times in 1,800 s.
3. Optional, heavy: the persisted nemesis over more seeds. Today only the replicated class has the 10,000-seed test
   (`AMPHORA_POWER_SEEDS=0:10000 cargo test -p amphora --lib -- --ignored
   a_member_power_cut_loses_no_acknowledged_replicated_write_over_10000_seeds`). A persisted twin would be
   `power_seeds(n, 0, 8, &Durability::Persisted)`.
4. A goal for a process disk budget (my message amphora-replica-log/disk-budget-goal/v1). It needs Ada's word on the
   per-replica bound meanwhile.

## Finding for the seam owner (answered: no hole; documented in 530091b)
A leader's tick makes a heartbeat answer that states a member's log position (`HeartbeatAnswers::Position`) count as
an append's answer (hyper-raft `handle_heartbeat_response`). In the driven harness that passes the pump's durable hold,
so the joiner test moves no clock. Whether such a position answer is durable at the member is the seam owner's
question. Its answer: such an answer is durable at the member by construction (hyper-raft handle_heartbeat;
hyper-durable's persisted messages), so the leak is the harness's, now documented.

## Seam owner's keys (amphora-seam-owner, codex-notices/t1-replica-log.reply.md)
- amphora-owner-t1-replica-log-design-20261109: direction agreed, seven points before code.
- amphora-owner-t1-replica-log-revision-20261109: revision agreed (all 7), five asks for the code notice.
- amphora-owner-t1-replica-log-92c5a9e-20261109: code agreed except two fixes, made in dce4aaa.
- amphora-owner-t1-replica-log-order-20261109: drafting-order question ruled (a); the code stands.
- amphora-owner-t1-replica-log-dce4aaa-20261109: dce4aaa agreed; rebase onto class-and-readers.
- amphora-owner-t1-replica-log-0e8a610-20261109: rebase 0e8a610 (tree 6d96c8fa) agreed, handed to the batch.
- (message, no key file) e5a6444 (tree afd2ef49) agreed, range 0e8a610..e5a6444, the joiner test included.

My notices: codex-notices/t1-replica-log.md, and the range-diff in codex-notices/t1-replica-log.range-diff.txt.

## Goals
- 7582f00a: said met (goals_done).
- aa81fbbf: code and tests done in 0e8a610. The term-churn measurement is left to you (item 2), so it is not said met.
