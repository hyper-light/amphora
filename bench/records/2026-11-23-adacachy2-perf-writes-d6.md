# perf/writes lane D6, adacachy2, 2026-11-23

Two A/Bs Ada asked to decide by data. adacachy2 (32 cores, btrfs), kiln `run` with needs
`linux/amd64,cpus=16,mem=24g`, release builds, one target dir. Other lanes' builds, tests and
benchmarks ran beside both (load and pressure recorded below), so the noise is real and large.

## 1. Reader checkout: pool (D4's fix, dd772668) against slots

Command (bench/checkout_ab.sh at dd772668; the script reads `--reuse` but never seeds the
database, so one seeding pass, 200,000 items, was added before the rounds):

    sed -e 's#amphora-perf-target/writes-bench#amphora-perf-target/writes#' \
        -e '/^args=(/a <one seeding run without --reuse>' bench/checkout_ab.sh > checkout_ab_seeded.sh
    JOBS=16 bash checkout_ab_seeded.sh

Records: adacachy2 `~/.cache/amphora-perf-bin/writes/checkout-ab-1234.jsonl`. 5 rounds, arm order
alternated, 5 s a run after 1 s warm-up, 0 errors in every run. Load (1 min) at each round's first
run: 20.6, 36.9, 34.8, 106.5, 112.2 (it reached 124.6). CPU pressure some avg60 4.9% before, 25.1%
after. The load came from other lanes (engine tests at 16 threads, tpch, postgres and kiln builds).

| op | clients | pool ops/s median [range] | slots ops/s median [range] | slots faster in | pool p99 / p99.9 ms | slots p99 / p99.9 ms | sys share pool / slots |
|---|---|---|---|---|---|---|---|
| point_pk | 1 | 265,800 [180,032-348,700] | 253,365 [246,846-338,372] | 2 of 5 | 0.005 / 0.011 | 0.005 / 0.011 | 0.19 / 0.18 |
| point_pk | 32 | 986,863 [614,519-1,404,785] | 1,600,578 [1,508,703-1,895,197] | 5 of 5 | 0.512 / 1.504 | 0.536 / 1.328 | 0.25 / 0.17 |
| point_pk | 128 | 742,179 [565,449-974,319] | 1,013,297 [839,312-1,113,666] | 4 of 5 | 3.424 / 7.744 | 2.496 / 5.568 | 0.22 / 0.23 |
| point_pk | 1024 | 472,934 [317,826-553,400] | 384,320 [322,141-537,580] | 1 of 5 | 14.336 / 25.856 | 34.816 / 68.608 | 0.39 / 0.36 |
| point_secondary | 1 | 270,431 [178,733-284,520] | 194,551 [167,739-267,831] | 2 of 5 | 0.005 / 0.009 | 0.007 / 0.012 | 0.12 / 0.14 |
| point_secondary | 32 | 738,791 [535,492-789,827] | 923,948 [799,006-1,083,420] | 5 of 5 | 0.432 / 1.104 | 0.536 / 1.184 | 0.22 / 0.16 |
| point_secondary | 128 | 454,923 [441,392-523,619] | 487,684 [444,105-643,176] | 3 of 5 | 4.864 / 9.856 | 1.680 / 3.264 | 0.18 / 0.30 |
| point_secondary | 1024 | 184,091 [95,067-319,872] | 204,274 [92,844-298,078] | 2 of 5 | 73.728 / 109.568 | 25.088 / 37.376 | 0.30 / 0.32 |

Latencies are medians over the 5 runs. At 32 clients the ranges don't overlap for either op
(rank-sum, 5 against 5, complete separation: p = 0.008): slots carry 62% more point_pk and 25% more
point_secondary there. At 1 and 1024 clients the medians point opposite ways for the two ops and
the ranges overlap; point_pk at 1024 has pool's p99.9 at 17-26 ms in the three quieter rounds
against slots' 55-118 ms, and the order flips in the two rounds at load above 100. Verdict: mixed
across client counts, so no pick (Ada's call). A rerun at 1 and 1024 clients on a quiet machine
would settle the ends.

## 2. Unit 3: the flusher's gather woken by commits (3d5d962d) against decision 14's timed gather (d5396fc2)

    W=~/.cache/writes-ab/u3ab2; cd $W && git checkout -q -f --detach 3d5d962d && \
      OPS=commit BASE=d5396fc2 FILES=crates/amphora/src/store.rs CLIENTS=1,8,32,128,512,1024 ROUNDS=5 \
      bash bench/records_ab.sh

Output: adacachy2 `~/.cache/amphora-perf-bin/writes/records-ab-u3-1243.txt`. 3 s a run, 0 failed
calls anywhere, load 14.9-22.1 (1 min), CPU pressure some avg60 15.1% at the start and 1.7% at the
end, I/O some avg60 0.8-4.1%. records_ab.sh runs `before` first in every round (not alternated).

| writers | timed gather, commits/s by round | median | commit-woken gather, by round | median | change |
|---|---|---|---|---|---|
| 1 | 125, 96, 147, 88, 87 | 96 | 147, 115, 106, 92, 70 | 106 | none measurable |
| 8 | 964, 772, 1149, 769, 851 | 851 | 913, 782, 430, 612, 518 | 612 | -28%, lower in 4 of 5 rounds |
| 32 | 3637, 2794, 4119, 2833, 2849 | 2849 | 2430, 1882, 1004, 1658, 2086 | 1882 | -34%, lower in 5 of 5, no overlap |
| 128 | 6408, 5997, 9409, 6399, 5243 | 6399 | 9074, 6991, 5584, 6293, 5886 | 6293 | none measurable |
| 512 | 16211, 20219, 25297, 18666, 24004 | 20219 | 20461, 21862, 22053, 21448, 17997 | 21448 | none measurable |
| 1024 | 26955, 26050, 33880, 25194, 30201 | 26955 | 26175, 33967, 28850, 25818, 27613 | 27613 | none measurable |

D5's risk holds: with 8 and 32 closed-loop writers the commit-woken gather loses a third of the
throughput, the same split decision 14 measured on fedora (7,390-7,605 a second gathering against
5,134-5,608 flushing at the first commit, at 32 writers). The batches handed over when gathering
begins are only the first few the last flush's senders send back, so the flush still begins near
the first commit. Unit 3 is not merged.
