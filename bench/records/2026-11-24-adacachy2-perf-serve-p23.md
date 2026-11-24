# Lane P23 (serve): the session worker against its parent in V13's real-time bench; the stage traced, nothing fixed (2026-11-24 UTC, adacachy2)

Goal: every replicated write either commits or gets a typed refusal, and no long write costs the group its leader
(CP-T1.13); replicated commits about 10x faster than rqlite and dqlite. Deliverable: the session worker's slowdown named
from a trace, removed and merged; if the gap doesn't reproduce, report that and stop.

Lane P23 ran from 05:20:51Z to 06:20:51Z (hour). Arms: H = main `3a58a13f` (its crates are `d0c5722d`'s), M = main's
parent before the landing, `6d8e7dcd`. H and M binaries: lane P22's release `amphora-bench` (built in its r349).

## Losses first

- **No fix landed; no product line changed.** The gap P22 saw (p50 1.09x to 1.63x, commits a second 0.79x to 0.93x) does not
  reproduce in 23 alternated rounds in each class (table below): H/M commits a second 1.00 (95% 0.85 to 1.17) persisted and
  0.97 (0.84 to 1.11) replicated. By the brief I stop there.
- What a 1.1x difference would need: the cells vary by 2x and more (the box at load 27 to 66, 0 to 17 of 32 cores idle; a round's
  two arms are 20 to 40 s apart), so 23 rounds resolve about 15%, not 5%. P22's "6 of 6" and "5 of 6" are not refuted by its own
  three rounds; they are not repeated by these 23 (H's p50 higher in 13 and 12 of 23, its commits lower in 12 and 14 of 23).
- P22's p50 is the pooled median of two populations (the leader's writes about 500 to 700 ms, the followers' 1,000 to 1,600 ms;
  half the samples each), so it jumps between modes: on P22's own files the pooled p50 is 1.39 and 1.42 times M's where the
  leaders' is 1.04 and 1.16, the followers' 1.02 and 1.19 (`pernode.py`).
- One M cell (a watched round, not in the 23: replicated, load 57) missed 7 writes' deadlines (1.8 commits a second, p50 9.6 s).
  Every one of the H cells and the 23 rounds' M cells: 0 refused, 0 misses.
- `b1`'s cells (r393) were overwritten on disk by `b2`'s (r413 ran the script's version before its `LDIR`): its numbers are the
  console's, kept in this record's table; its per-node files are gone.
- Calibration (RULES 5e) was not shared between the arms; it is the planner's CPU costs, and V13's inserts' plans were not read.
- The trace names a stage and not all of its why: the forward waits out a log write (measured); why a write the owner begins while a session is out
  holds it 3 ms longer than one begun before is not traced (the write's bytes weren't logged).
- Not done: a fix (below), an A/B of one, a macOS run, a samply profile (the cells' time is waiting, not CPU: the replica threads
  use 0.05 to 0.07 of a core a member; the bench's relay pumps spin at about one core a member).
- About 5.3 GB of traced binaries of mine are left on adacachy2 (`~/.cache/amphora-perf-bin/serve-p23`), to be removed after
  Ada's benchmark window (it began at 05:56Z; I have run nothing there since).

## Step 1: V13's real-time bench, H against M

Lane P22's cell (`cell13.py`: 32 spread writers over 3 members, batch 1, 600 B, 1 s warmup, 5 s counted, `--deadline-s 10`,
nice 10), arms alternated, the order of the arms turned each round; `b1.sh`, `R` rounds. `refused` and `misses` are 0 in every cell
of the 23 rounds.

| set | when, load, CPU pressure | rounds | class | commits/s H, M (medians) | H/M (95%) | p50 ms H, M (medians) | H/M (95%) |
|---|---|---|---|---|---|---|---|
| b1 (r393) | 05:22Z, load 50 to 61, cpu-some 14 to 48, io-some 4 to 18 | 5 | persisted | 19.4, 15.6 | 1.12 (0.86 to 1.48) | 1770, 1917 | 0.97 (0.81 to 1.15) |
| | | 5 | replicated | 15.0, 18.0 | 0.91 (0.69 to 1.22) | 1688, 1344 | 1.11 (0.72 to 1.71) |
| b2 (r413) | 05:28Z, load 40 to 66, cpu-some 11 to 50 | 6 | persisted | 18.5, 19.4 | 0.92 (0.73 to 1.17) | 1704, 1638 | 1.09 (0.73 to 1.63) |
| | | 6 | replicated | 15.2, 17.9 | 0.78 (0.62 to 0.97) | 1868, 1335 | 1.25 (1.04 to 1.50) |
| b3 (r451) | 05:37Z, load 47 to 66, cpu-some 13 to 55 | 6 | persisted | 20.9, 18.9 | 1.05 (0.61 to 1.80) | 1647, 1679 | 1.10 (0.53 to 2.27) |
| | | 6 | replicated | 18.2, 15.7 | 1.17 (0.85 to 1.60) | 1802, 1909 | 0.86 (0.64 to 1.15) |
| b4 (r485, needs cpus=6) | 05:44Z, load 28 to 43, cpu-some 4 to 35 | 6 | persisted | 37.5, 39.7 | 0.93 (0.81 to 1.06) | 954, 803 | 1.19 (0.98 to 1.44) |
| | | 6 | replicated | 36.9, 37.2 | 1.05 (0.86 to 1.27) | 778, 668 | 1.02 (0.75 to 1.37) |
| **all** | | **23** | **persisted** | **20.6, 20.4** | **1.00 (0.85 to 1.17)** | 1606, 1507 | 1.09 (0.88 to 1.35) |
| **all** | | **23** | **replicated** | **18.6, 18.8** | **0.97 (0.84 to 1.11)** | 1507, 1344 | 1.05 (0.90 to 1.22) |

The H/M columns are the geometric mean of the rounds' ratios and its 95% interval (two standard errors of the log ratio). H's
commits are lower than M's in 12 of 23 rounds (persisted) and 14 of 23 (replicated); H's p50 is higher in 13 and 12. In the only
quiet set (b4, the box at P22's rates, 26 to 44 commits a second) H/M is 0.93 and 1.05. The gap is not there to trace as a whole-cell
difference.

## Step 2: traced anyway, in the same cell, two ways

`diag.py` (the lane's, never for main) puts a timestamp (with its thread's CPU and run-queue wait) at each stage in the owner, the
worker and the leader's slots, in release `amphora-bench`; `cell23t.py` keeps each member's lines, `trace.py`, `trace2.py`,
`trace3.py` read them.

1. H against M, 3 rounds a class a set (`t2`, noisy box; `q2`, quieter). The hand-off's own stages, H persisted: `hand_begin` to
   `hand_flushed` 0.08 ms mean, `handing` to `w_begin` (the worker's wake) 0.1, the session on the worker 0.2 to 0.3 (the same work as M's inline 0.2 to 0.35), `w_end` to `taken` (the owner's wake) 0.1, `taken` to `decide` 0.02, `decide` to `proposed` 0.01. All of the
   worker's path, `go` to `proposed`, is 0.85 ms median against M's 0.50 persisted, 0.97 against 0.47 replicated: +0.35 to 0.5 ms on a slot cycle of 50 to 110 ms.
2. Inline and worker in one run, alternated write by write (arm `diag-H2`, `P23_ALT=1`: every second application write runs inline on
   its owner, as M does, the others on the worker; `t4.sh`, so both see the same load and the same cell). The leader's cycle, slot start
   to slot start, by the mode of the write that held the slot (mean ± its error, median, ms):

| set | class | inline | worker | worker less inline |
|---|---|---|---|---|
| t4 (load 50 to 63) | persisted | 66.9 ± 3.3, 55.2 | 70.5 ± 4.4, 55.5 | +3.6 ± 5.5 |
| t4 | replicated | 80.9 ± 4.3, 64.9 | 83.1 ± 4.3, 63.8 | +2.3 ± 6.1 |
| q4 (load 38 to 42) | persisted | 51.6 ± 1.6, 47.7 | 53.1 ± 3.4, 47.2 | +1.4 ± 3.8 |
| q4 | replicated | 54.0 ± 1.3, 53.2 | 57.8 ± 1.5, 56.1 | **+3.8 ± 2.0** (7%) |
| t5 (load 38 to 42) | persisted | 45.2 ± 0.8, 43.4 | 45.5 ± 1.1, 43.1 | +0.4 ± 1.4 |
| t5 | replicated | 49.8 ± 2.7, 48.8 | 58.2 ± 8.7, 50.6 | +8.4 ± 9.1 (one 0.8 s slot; medians +1.8) |

   The persisted class shows no difference. In the replicated class the worker's slots are longer by 2 to 4 ms of
   49 to 57 (4 to 7%) in the two quiet sets (medians +2.9 in q4, +1.8 in t5) and by nothing the loaded one resolves (medians -1.1, t4).

3. The stage the difference is in: **`prop>out`, the follower's forward (its `MsgPropose`) leaving**. Replicated class, medians
   (`q4`): inline 8.6 ms, worker 13.1; (`t5`) inline 8.1, worker 9.3; (`t4`, loaded) 13.6, 14.7. Persisted: 0.1 to 0.2 inline, 0.01 worker (the
   worker's path proposes and steps again at once, `driving`). Every other stage of the write is equal within its error.
   `t5` also logged each log write's start and end (`hyper-durable`'s `take_ready` and `durable`): in the replicated class a write of the
   follower's own log is out when the forward is proposed in 77 of 93 inline writes and 89 of 94 worker writes, and the forward leaves 0.03 ms (median) after that write ends (so it waits that write out). Inline, that write began before the session in 77 of 77 (age at the proposal 0.7 ms median); on the worker, **it began while
   the session was out in 57 of 89** (0.2 ms median after the session's start; age at the proposal 0.5 ms). The forward's wait is then 12.0 ms median (57 writes) against 9.2 ms
   (the 32 writes whose in-flight write began before the session) and 8.3 ms inline. In the persisted class no write is out at the proposal
   (109 of 109 inline, 108 of 110 worker), so nothing waits.

   So the stage is the brief's second: the owner's step while a session is out. The owner goes on stepping (by design) and begins
   a log write; a follower's queued messages, its forward among them, leave only when no write of its is out
   (`RawNode::releases_now` in `vendor/hyper-raft/crates/hyper-raft/src/node.rs`, the rule for a member that is no leader), so the forward
   waits out the write. The forward waits behind its own log write in both arms (inline 8 ms, M 12 and 17 ms in `q2` and `t2`);
   on the worker the write it
   waits for is, in 57 of 89, one the owner began while the session was out, and that wait is 12.0 ms median against 9.2 for the 32 begun before it
   and 8.3 inline. It is not the write's age (0.2 to 0.7 ms apart): why a write begun in the session holds the forward longer is not traced (`t5` logs
   each write's start, end and message count, not its entries' bytes; its files are on adacachy2, in Ada's window). The stage's median difference, 1.2 to
   4.5 ms, is the size of the replicated cycle's (+2 to 4 ms); that it is all of it is inferred, since the two were measured in the same runs.

## The fix, not made (for the successor; it needs the coordinator's word)

A forward promises nothing the member's disk holds, so it need not wait for a write. `RawNode::messages_now` and `light_now` (the
lane V3 path, `0009`) could release a follower's `MsgPropose` (and a `MsgReadIndex` it forwards) whatever its writes out, as etcd's raft
does: there only `MsgAppResp`, `MsgVoteResp` and `MsgPreVoteResp` wait for the append (from memory, `raft.go`'s `send`; check it before
citing). It would be a new vendored patch, `0011`, opt-in and off by default as `0009`'s `answer_beats_now` is (`Config`, `raft.rs:298`,
Amphora turns it on at `replicated.rs:2898`), general, and goes upstream or not by Ada's word. It takes the forward's wait (8 to 14 ms
of a 50 to 57 ms replicated slot cycle, both arms) to nothing; the difference of the worker from inline goes with it. Not built, not
measured: a guess at its size by the stage's, and a change to hyper-raft's core with its own suite. Rule 0 (revised 11-24) says no
performance work starts while main has known correctness defects; this lane stops.

Found on the way, one line each: (1) the forward and the barrier's `MsgReadIndex` waiting behind a log write in the replicated class, in
both arms (above); (2) the bench's relay pumps spin (`std::thread::yield_now`) at about one core a member, so a cell is more than the
2 cores `needs` asked (3 pumps and a driver), which is why `b4` asked 6; (3) one hand-off (`t2`, H persisted, its first cell) spent
13 ms blocked in `hand_off`'s `flush()` (`hand_begin` to `hand_flushed`, CPU 0.4 ms; a log restart there is inferred, not traced); the longest of the
687 followers' hand-offs of t4, q4 and t5 was 1.4 ms.

## Runs

| run | needs | what |
|---|---|---|
| r393 | cpus=2,mem=8g | b1: step 1, 5 rounds, then 2 rounds with each member's threads' CPU and run-queue wait (`cell23.py`) |
| r413 | cpus=2,mem=8g | b2: step 1, 12 rounds asked, stopped by me after 6 and a half for the traced cells |
| r412, r459, r525 | cpus=4,mem=12g, jobs 4 | t1.sh: the traced arms (diag-M 81 s, diag-H 63 s, diag-H2 82 s and 66 s), in lane P20's shared clone and the serve lanes' target dir |
| r429, r500 | cpus=2 and cpus=4,mem=8g | t2 (3 rounds, loaded), then q4 and q2 (4 and 3 rounds, quieter) |
| r462, r525 | cpus=2 and cpus=4 | t4 (4 rounds, loaded), t5 (3 rounds, log writes traced) |
| r451, r485 | cpus=2 and cpus=6,mem=8g | b3 and b4: step 1, 6 rounds each |

All on adacachy2 (00aad8da...) through kiln `run`, sync false, before 05:56Z. Scripts and analyses:
`bench/records/2026-11-24-adacachy2-perf-serve-p23/` on branch perf/serve-p23.
