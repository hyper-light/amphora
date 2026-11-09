# PL.1's oracle campaigns: CERT and the order check on decision 18 (2026-11-06)

What: `bench/oracles/nightly.sh` (every oracle, 1,800 s, `--reduce-seconds 20`, the reference SQLite
3.54.0 that `bench/oracles/sqlite.sh` builds) on a release build of `amphora-bench` at 8567e7b
(decision 18, join sizes from equivalence classes), seeds 11 and 12, as decision 13's campaigns
ran (`2026-10-26-adas-mac-studio-2-order-check/`). Every CERT or order finding was to be replayed
with `--replay-check` on a release build of the parent, eb999f8, built with the change's resolved
`bench/Cargo.lock` (bench is a workspace of its own and commits none). There was none to replay.

Where: adalundhe-macbook-pro (Apple M5 Max, 64 GiB, macOS 26.5.2), 2026-11-06, 09:53 to 10:53
PST. The load was 18.6 at seed 11's start and 6.6 at seed 12's. The oracles need no quiet machine:
they count checks, not time. It ran there because adas-macbook-pro-2's disk filled during the
build, fedora did not answer, and Mac Studio 3 was at load 47. The summaries are on that host
under `~/.cache/planner-stats/oracles/seed{11,12}/summary.json`; the script is
`~/.cache/planner-stats/oracles.sh` there.

| seed | databases | checks, every oracle | findings | cert: checks, passed, failed | order: checks, passed, failed |
|---:|---:|---:|---:|---|---|
| 11 | 55,855 | 2,263,778 | 0 | 279,275, 279,246, 0 (29 SQLite refused) | 279,275, 279,275, 0 |
| 12 | 52,315 | 2,120,532 | 0 | 261,573, 261,538, 0 (35 SQLite refused) | 261,573, 261,573, 0 |

No check of any oracle failed in either run: diff, dqp, eet, norec, pqs, same and tlp have 0
failed beside cert and order. Neither CERT oracle skipped a check for want of an estimate, where
decision 13's campaigns on d762dc2 skipped 46,276 of 502,632 at seed 11.

So the change brings no CERT or order failure (T-PL.1.1; decision 18's falsifier), in 540,848
CERT checks and 540,848 order checks.
