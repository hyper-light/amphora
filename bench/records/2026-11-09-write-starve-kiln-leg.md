# The follower-write candidates in kiln's real-process promotion test (2026-11-09)

Step 5 of kiln plan 01a1b8d174dc72d18b9d2289b6e8fe11 (goal 9c63f7d4), its first leg. **Not a benchmark** (CLAUDE.md
§7): debug builds, on a host other work was loading (load 3 to 24), one to three runs each. On Ada's word (2026-11-09,
through the Amphora agent) the release benchmarks under the heavy lock and further repeats are the Amphora agent's; this
records what the implementing agent ran before that.

- **Host:** adas-mac-studio.local-2 (runner d1323898), 2026-11-09 16:04Z to 16:55Z.
- **kiln:** p2p/stall at 3aa1cb35097 (`git archive` of ~/.cache/kiln-p2p/stall/wt, its tree untouched), its vendored
  Amphora (f99b758 with kiln's five patches) carrying t1/write-starve's admission changes (9416f2d..4b8c510), the
  floor session's tail rule ported to it (that copy predates the floor). The scratch repository is
  ~/.cache/write-starve-kiln on adacachy (commit 7ad48c5), synced to the studio.
- **Test:** `cargo test --locked -p kiln --test p2p_group -- --test-threads=1 --exact
  p2p_a_promotion_to_voter_costs_the_groups_writes_no_gap_past_an_election_timeout`, the admission chosen by
  `AMPHORA_ADMISSION_PROTOTYPE` (crates/amphora/src/replication/admission.rs), the runners' logs copied out each second
  while it ran (kiln logs `write_again` at each power of two).

| admission | run | both runners voters | box-a write_again (max) | box-b write_again (max) | unknown outcomes | longest gap before any promotion | longest gap across a promotion | load |
|---|---|---|---|---|---|---|---|---|
| exact (today, the control) | 1 | no: a runner joined again (3 joins), voters [1] | 128 | 128 | 2 | n/a | n/a | 7.1 |
| exact | 2 | no: the same | 128 | 128 | 2 | n/a | n/a | 4.8 |
| exact | 3 | no: the same | 128 | 128 | 1 | n/a | n/a | 4.6 |
| slot (C) | 1 | yes | 2 | 2 | 1 | 1.32 s | 4.27 s | 3.3 |
| slot | 2 | yes | none logged | none logged | 0 | 1.71 s | 1.28 s | 5.6 |
| slot | 3 | yes | 4 | none logged | 0 | 1.46 s | 1.38 s | 3.9 |
| exact-then-slot (G) | 1 | yes | 8 | 16 | 0 | 1.35 s | 0.60 s | 4.0 |
| exact-then-slot | 2 | yes | 8 | 8 | 0 | 1.04 s | 0.71 s | 4.6 |
| pages-then-slot (D) | 1 | yes | 1 | 1 | 0 | 0.74 s | 0.71 s | 4.9 |
| pages-then-slot | 2 | yes | 2 | none logged | 0 | 0.45 s | 0.66 s | 3.1 |
| pages (A1m) | 1 | yes | 2 | 1 | 0 | 0.84 s | 0.84 s | 4.2 |
| pages | 2 | yes | 1 | 2 | 0 | 0.51 s | 3.61 s | 24.3 |

(The third exact-then-slot run was stopped when the heavy runs moved to the Amphora agent; it is not counted.)

What it shows:
- **The control reproduces kiln's finding** three times of three: the runners' `write_again` reaches 128, their writes
  end unknown, a runner joins again, and neither becomes a voter.
- **Every candidate ends the starvation:** both runners become voters every run, `write_again` at most 16 (G) and at most
  2 under D and A1m, against 128. Unknown outcomes: one, in C's first run.
- **kiln's own assertion fails under every candidate, and is kiln step 10's question, not this goal's.** The test asks
  that no promotion stand the group's writes still past the election timeout (200 ms). Gaps of 0.45 s to 1.71 s appear
  **before any promotion** in every candidate's runs, where no promotion is under way, so debug builds and the host's
  load account for gaps of that size whatever the admission. C makes them worse across a promotion (1.28 s to 4.27 s:
  the leader holds its writes for each runner's ReadIndex); A1m's 3.61 s came at load 24.
