# The follower-write rules in kiln's real-process promotion test, release (2026-11-12)

Goal 9c63f7d4 (follower writes starve under steady leader writes; CP-T1.8), the release A/B the debug leg
(2026-11-09-write-starve-kiln-leg.md) left to the Amphora agent. Ada chose "G now, D next" (2026-11-12) on this table.
Three rules: **exact** (today: a write applies only on the entry before it), **exact-then-slot** (G: exact, and the
leader holds its own writes for a member refused once) and **pages-then-slot** (D: page read-set validation with page
1's size merged, and the slot after two refusals in a row); docs/research/follower-write-starvation.md says what each is.

**The load gate was off** (Ada, 2026-11-11: "Just run the fucking bench christ"): runs began at 1-minute loads of 2.30
to 3.59 where the load was kept, over CLAUDE.md §7's bar of 2, and the host ran kiln CI work beside them. Interleaved
rounds put each rule under the same conditions, so the rules compare with each other; the absolute gaps are not a quiet
host's.

- **Host:** kiln runner cec37f60f22c5a60d27f08f4d2e5daa8, labelled adas-mac-studio.local (its hostname reports
  Adas-Mac-Studio-3.local). Apple M4 Max, 14 cores, 38654705664 bytes (36 GiB) of memory, macOS 26.2 (25C56), from
  `sysctl -n machdep.cpu.brand_string hw.ncpu hw.memsize` and `sw_vers`.
- **kiln:** the scratch copy 7ad48c5 (kiln p2p/stall 3aa1cb35097 with t1/write-starve's prototypes, the rule chosen by
  `AMPHORA_ADMISSION_PROTOTYPE`; the Amphora side is t1/write-starve at 044c44f's code, which stays off the batch).
- **Build:** `cargo test --release --locked -p kiln --test p2p_group`, the `release` profile (build.log of each set).
- **Test:** `p2p_a_promotion_to_voter_costs_the_groups_writes_no_gap_past_an_election_timeout`, `--test-threads=1
  --exact --nocapture`, a server and two runners (box-a, box-b) as separate processes, writing through the server.
- **Script:** probe carrier `ws-ab.sh` (commits 6705c38, then 43b4534, which keeps the console and each run's start
  load). It copies the runners' logs out of the test's tempdir each second; kiln logs `write_again` (writes evaluated
  again since the process started) at each power of two, so a maximum is the last power of two reached.
- **Set 1:** ~/.cache/ws-ab on that host, 5 interleaved rounds, 2026-11-12 03:57Z to 04:31Z (test.log times), script
  6705c38. **Each run's start load was not kept:** that version printed it only to its console, and kiln collected the
  run's output before it was read.
- **Set 2:** ~/.cache/ws-ab-2, 3 interleaved rounds, 05:08Z to 05:28Z, script 43b4534, start loads in console.log and
  each run's load.txt. It was killed at round 4, when the Amphora agent's kiln session was lost; no round-4 run exists.
- 8 runs per rule. The logs were read in place, read-only, with kiln's run tool (sync false); nothing was removed.

## Every run

Columns: whether both runners became voters (and the failure line); each runner's `write_again` maximum (— means no
`write_again` line in its log); the runners' unknown commit outcomes (`commit outcome is unknown` lines); the writer's
line `election timeout 200ms; N commits, M writes not committed, over S`; the longest gap before any promotion and across
each promotion (the gap, over how many commits); the test's own time.

Under **exact** the test fails at `a runner joined the group again before every member voted (3 joins): Some([1])`
(voters [1], the server alone) and returns before it prints the writer's line or any gap, so those columns are n/a.
Under G and D every run fails at crates/kiln/tests/p2p_group.rs:608, `a promotion stood the group's writes still past
its election timeout 200ms`.

| set | rule | round | load | voters | box-a again | box-b again | unknown (a/b) | commits | not committed | span | before any promotion | first promotion | second promotion | test |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | exact | 1 | not kept | no: joined again (3 joins), [1] | 256 | 256 | 2 in all | n/a | n/a | n/a | n/a | n/a | n/a | 75.08 s |
| 1 | exact | 2 | not kept | no: the same | 256 | 256 | 1/1 | n/a | n/a | n/a | n/a | n/a | n/a | 75.15 s |
| 1 | exact | 3 | not kept | no: the same | 256 | 256 | 1/1 | n/a | n/a | n/a | n/a | n/a | n/a | 76.89 s |
| 1 | exact | 4 | not kept | no: the same | 256 | 128 | 1/1 | n/a | n/a | n/a | n/a | n/a | n/a | 77.16 s |
| 1 | exact | 5 | not kept | no: the same | 128 | 128 | 2/1 | n/a | n/a | n/a | n/a | n/a | n/a | 79.83 s |
| 2 | exact | 1 | 2.73 | no: the same | 256 | 256 | 1/1 | n/a | n/a | n/a | n/a | n/a | n/a | 77.26 s |
| 2 | exact | 2 | 3.30 | no: the same | 256 | 256 | 1/1 | n/a | n/a | n/a | n/a | n/a | n/a | 77.80 s |
| 2 | exact | 3 | 3.59 | no: the same | 256 | 256 | 1/2 | n/a | n/a | n/a | n/a | n/a | n/a | 77.56 s |
| 1 | G | 1 | not kept | yes, [1]→[1,2]→[1,2,3] | 8 | 8 | 0/0 | 114 | 0 | 17.41 s | 829.8 ms (66) | 443.0 ms (8) | 497.0 ms (41) | 26.61 s |
| 1 | G | 2 | not kept | yes, [1]→[1,2]→[1,2,3] | 8 | 4 | 0/0 | 121 | 0 | 18.52 s | 577.9 ms (65) | 616.9 ms (7) | 616.9 ms (50) | 27.79 s |
| 1 | G | 3 | not kept | yes, [1]→[1,2]→[1,2,3] | 4 | 4 | 0/0 | 123 | 0 | 18.78 s | 504.1 ms (64) | 649.4 ms (6) | 649.4 ms (54) | 30.00 s |
| 1 | G | 4 | not kept | yes, [1]→[1,3]→[1,2,3] | 8 | 8 | 0/0 | 140 | 0 | 20.58 s | 366.0 ms (78) | 869.1 ms (5) | 869.1 ms (58) | 32.01 s |
| 1 | G | 5 | not kept | yes, [1]→[1,3]→[1,2,3] | 8 | 8 | 0/0 | 151 | 0 | 25.38 s | 461.0 ms (82) | 968.7 ms (5) | 620.0 ms (65) | 37.16 s |
| 2 | G | 1 | 2.78 | yes, [1]→[1,2]→[1,2,3] | 4 | 8 | 0/0 | 136 | 0 | 20.98 s | 701.0 ms (70) | 742.6 ms (6) | 635.7 ms (61) | 32.24 s |
| 2 | G | 2 | 2.70 | yes, [1]→[1,2]→[1,2,3] | 8 | 8 | 0/0 | 138 | 0 | 21.10 s | 753.9 ms (70) | 955.8 ms (7) | 605.7 ms (62) | 32.35 s |
| 2 | G | 3 | 3.35 | yes, [1]→[1,2]→[1,2,3] | 8 | 4 | 0/0 | 121 | 0 | 18.47 s | 702.1 ms (66) | 870.5 ms (5) | 568.1 ms (51) | 30.34 s |
| 1 | D | 1 | not kept | yes, [1]→[1,2]→[1,2,3] | 2 | 1 | 0/0 | 184 | 0 | 23.91 s | 195.9 ms (109) | 621.1 ms (6) | 621.1 ms (70) | 33.51 s |
| 1 | D | 2 | not kept | yes, [1]→[1,2]→[1,2,3] | 1 | — | 0/0 | 137 | 0 | 19.40 s | 314.1 ms (83) | 594.1 ms (5) | 594.1 ms (50) | 30.46 s |
| 1 | D | 3 | not kept | yes, [1]→[1,2]→[1,2,3] | 1 | 1 | 0/0 | 114 | 0 | 16.77 s | 278.2 ms (75) | 489.7 ms (6) | 573.8 ms (34) | 27.93 s |
| 1 | D | 4 | not kept | yes, [1]→[1,2]→[1,2,3] | 2 | 1 | 0/0 | 108 | 0 | 15.97 s | 329.0 ms (70) | 414.0 ms (7) | 496.9 ms (32) | 28.12 s |
| 1 | D | 5 | not kept | yes, [1]→[1,2]→[1,2,3] | 2 | 1 | 0/0 | 126 | 0 | 18.51 s | 295.1 ms (78) | 475.9 ms (6) | 422.1 ms (43) | 30.30 s |
| 2 | D | 1 | 2.39 | yes, [1]→[1,2]→[1,2,3] | 1 | 2 | 0/0 | 122 | 0 | 16.94 s | 348.0 ms (75) | 348.0 ms (7) | 475.0 ms (41) | 27.61 s |
| 2 | D | 2 | 3.00 | yes, [1]→[1,2]→[1,2,3] | 1 | — | 0/0 | 222 | 0 | 26.29 s | 172.0 ms (136) | 338.0 ms (7) | 784.2 ms (80) | 37.38 s |
| 2 | D | 3 | 2.30 | yes, [1]→[1,2]→[1,2,3] | 2 | — | 0/0 | 121 | 0 | 17.57 s | 324.0 ms (75) | 670.0 ms (5) | 494.9 ms (42) | 28.59 s |

Set 1's exact round 1: its box-a.log and box-b.log are another run's (their lines are dated 2026-11-09 19:31Z: the
copy loop took an older tempdir's logs over this run's). Its row is from test.log, which carries both runners' own
lines for this run (2026-11-12 03:58Z): two `write_again` series, each reaching 256, and two unknown outcomes between
them, not attributable to a runner. Where one gap is the same across both promotions (G rounds 2 to 4, D rounds 1 and 2
of set 1), one stall spanned both promotions' windows.

## Per rule (8 runs each)

Gaps are best (least) / median / worst of the 8 runs; "worst promotion gap" is each run's larger of its two promotion
gaps. Commits a second is the writer's commits over its span.

| rule | both runners voters | write_again max per runner (16) | unknown outcomes | writes not committed | commits a second | gap before any promotion | worst promotion gap | runs under 200 ms before any promotion | runs passing the 200 ms assertion | test time |
|---|---|---|---|---|---|---|---|---|---|---|
| exact (today) | 0 of 8 | 256 in 13, 128 in 3 | 18 in 8 runs, every run | n/a | n/a | n/a | n/a | n/a | 0 of 8 (not reached) | 75.1 to 79.8 s |
| G (exact-then-slot) | 8 of 8 | 4 to 8 | 0 | 0 | 5.95 to 6.80 | 366.0 / 639.4 / 829.8 ms | 497.0 / 805.9 / 968.7 ms | 0 of 8 | 0 of 8 | 26.6 to 37.2 s |
| D (pages-then-slot) | 8 of 8 | none to 2 | 0 | 0 | 6.76 to 8.45 | 172.0 / 304.6 / 348.0 ms | 475.0 / 584.0 / 784.2 ms | 2 of 8 | 0 of 8 | 27.6 to 37.4 s |

## What it shows

- **Today's rule starves in release as it did in debug**, 8 runs of 8: each runner's `write_again` reaches 128 or 256,
  every run ends commit outcomes unknown, a runner joins again, and neither runner becomes a voter. Release did not
  change the finding.
- **G and D each end the starvation**, 8 runs of 8: both runners become voters, no unknown outcome, no write left
  uncommitted. Under G a runner's write is evaluated again at most 8 times; under D at most 2, and in 3 of 16
  runner-runs never.
- **D stalls the group's writes less than G** in this test: its longest gap before any promotion is about half G's
  (median 305 ms against 639 ms) and its worst promotion gap lower (median 584 ms against 806 ms), and the writer
  commits faster under it. These are interleaved runs on one host, so the comparison holds across the load both saw; at
  8 runs a rule and loads over 2 it is not a §7 benchmark.
- **kiln's 200 ms gap assertion fails under every rule.** Under G and D it fails in every run (16 of 16), at
  p2p_group.rs:608; under exact the test fails before reaching it, so no run of any rule passes it. Gaps over the 200 ms
  election timeout appear **before any promotion** in all of G's runs and 6 of D's, where no promotion is under way, so
  what holds the group's writes past the election timeout is not only the promotion. Whether that assertion's bound is
  the right one, and what stalls the writes, is **kiln step 10's question**, not this goal's.
- The model's runs (2026-11-07-write-starve-model.md, `crates/amphora/tests/write_starve_model.rs`) put D slower than G
  on same-table appends; this test's writes are not that workload, and D's lead here does not contradict it.

## Falsifier

A quiet-host rerun (load under 2, best of N, CLAUDE.md §7) in which G's gaps match or beat D's, or in which either rule
leaves a runner unpromoted or an outcome unknown, overturns the rows above.
