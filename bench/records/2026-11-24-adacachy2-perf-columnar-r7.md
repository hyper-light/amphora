# Lane R7: cb4_ab's waves at scale 1, and R6's A/B on S8's analytics statements (2026-11-24, adacachy2)

Ada's goal is Amphora about ten times faster than DuckDB and SQLite on analytics. R6 made the
planner time the rows' path the way it times the column copy's (merged as 50ab86c6). This lane had
two deliverables: cb4_ab passing at scale 1 with its waves scenario reworked, then R6's A/B on S8's
analytics statements with Turso and SQLite in the same rounds.

Verdict, losses first.
- cb4_ab is not fixed, and the decided rework (press round 2 below what the copies' catch-up
  takes) cannot make a copy let go. The catch-up's takes are paid by the queue bytes the same call
  gives back first, to the same source, so no press level refuses them. Read from the code (below);
  not measured. The run that would measure the rounds (r1958, macOS) was still queued at the end
  of the hour.
- The A/B cannot show R6's fix on these statements: the copies priced none of them. With
  `AMPHORA_STORAGE=versioned AMPHORA_COLUMN_PATH=on` and `AMPHORA_PLAN_TRACE=1`, neither arm made a
  single copy-or-rows choice for any of the five statements at 1 or 32 clients (0 trace events per
  arm, r1951). R6's change prices that choice, so both arms ran the same path.
- No measurable difference between the arms on any statement (B/A 0.77x to 1.10x, every pair's
  5-round ranges overlapping), as expected when neither arm chose a path.
- Amphora, copies on (arm B), is behind SQLite on 8 of the 10 statement and client cells by median,
  1.25x to 67x (in one, `aggregates_range` at 32 clients, the 5-round ranges overlap), and behind
  Turso on 8 of 10 (in one, `table_group_by` at 32 clients, within noise). It leads only
  `table_group_by` at 1 client (1.36x SQLite, ranges touching; 1.90x Turso), `join_three` at 1
  client against Turso (2.98x), and `table_group_by` at 32 clients against SQLite, which finished
  none in its 2 s window. The 10x goal is far off on these
  statements.
- Errors 0 on every engine, and each statement's answers were the same on A, B, Turso and SQLite.
- The causes of Amphora's gaps were not profiled this hour, so the gaps are measured but not
  explained.

## 1. cb4_ab's waves scenario at scale 1

The waves check fails because no copy is ever let go (`dropped` 0). R6 found that round 2 is the only
pressed round at scale 1 in which a copy is read. The reworked scenario was to press round 2 below
what the copies' catch-up takes. The code shows that cannot refuse a catch-up:

- `press` (`columns_sql.rs`, test helper) leaves the copies `free` bytes in all, what they hold
  included: the room left is `free - held`, floored at 0. After the wave the copies hold what the
  wave bought, each about one `need`, already past round 2's half a copy, so round 2 leaves them
  0 bytes of new room. Inferred from R6's counts (copies read in round 2), not measured; r1958
  records each round's `held`.
- The table's growth is written with the room given back (lane colcopy-8), so the commit's queue
  take (`Feed`, `columns.rs`, `self.room.take(QUEUE, ...)`) is granted, and the queue holds the
  growth's bytes through the press.
- `Columns::catch_up` (`columns.rs`) drains the queue and gives its bytes back
  (`self.feed.released(freed)`, which is `room.give`) before it applies a single change.
- Each change applied takes exactly its `change_bytes` (`Generation::apply`, `Grant::take`, which
  takes exact bytes with no rounding up), the same bytes the queue took for it.
- A bought copy's queue and generations take from one source: `Feed::new(tree, self.room.clone(),
  ..)` and `Columns::new(feed, self.room.clone())` (`columns_sql.rs`). A copy waiting under policy
  B has a feed on `memory::capped(0)` instead, so it queues nothing while it waits.

So in this single-threaded harness a catch-up is always granted, at any press level, 0 included.
A let-go at scale 1 needs a take in a pressed round that no release pays for. Two exist:
- a rebuild, which runs when the copy is behind or when its measured delta cost passes its last
  rebuild's (`Columns::wants_rebuild`). Measured costs make the second one depend on timing.
- a commit that finds the copies' room full, which marks the copy behind (decision 1, item 2 of
  `docs/design/columnar.md`). The harness gives all room back for writes, so its commits never do.

Making the commits in a pressed round meet the copies' room, while the rows' own takes still have
room, needs the test's source to tell the copies' takes from the rows'. `Source::take` names its use,
but `Source::give` does not, so this is a change to the harness's source, not to the scenario's
numbers. It is a decision for the coordinator, not done here (rule 0).

## 2. R6's A/B on S8's analytics statements

### Setup

- adacachy2 (runner 00aad8da), 32 cores. Build and load: kiln r1951, needs `cpus=4,mem=12g`.
  Timed rounds: kiln r1971, needs `cpus=16,mem=24g`, granted at the first ask.
- Arm A 3c0e1c74 (main 0bb4b917 with R6's trace event). Arm B 44f70ed2 (arm A plus R6's fix).
  `bench/` built `--release` (the bench workspace's own profile) in one target dir, arm A then arm
  B on the lock arm A's build resolved. Binaries: `~/.cache/amphora-perf-bin/columnar/<sha>/`.
- Amphora: `AMPHORA_STORAGE=versioned AMPHORA_COLUMN_PATH=on`, `--suite --target amphora
  --items 200000`, loaded once by arm B and copied for arm A. One calibration for both arms: arm
  B's measured body copied under arm A's key (body sha256 5af22ad1fe82f858 for both, RULES 5e).
- Turso and SQLite: the same suite and items on arm B's binary (the arms differ only in the
  engine), each loaded once.
- Statements: `table_count`, `table_group_by`, `filter_no_index`, `aggregates_range`,
  `join_three`, at 1 and 32 clients, 2 s timed after 1 s of warmup, `--reuse`.
- 5 rounds. Odd rounds run A, B, Turso, SQLite; even rounds run SQLite, Turso, B, A.
- Script: `~/.cache/amphora-perf/reals-r7-scripts/ab7_linux.sh` on adacachy (R6's
  `ab_linux.sh` with Turso and SQLite added, the build's exit code checked, and the trace switch
  set in the trace pass; R6's pass never set `AMPHORA_PLAN_TRACE`). Commands:
  `ab7_build_cmd.txt` (ROUNDS=0 JOBS=4) and `ab7_cmd.txt` (ROUNDS=5 JOBS=16).

### Paths each statement took (r1951, traced pass, untimed)

| statement | clients | arm A choices | arm B choices |
|---|---|---|---|
| table_count, table_group_by, filter_no_index, aggregates_range, join_three | 1 and 32 | none | none |

0 `amphora::column_path` events on either arm. The traced pass ran every statement (its `==
amphora <statement> clients=<n>` lines are there), with the subscriber on. Why the copies never
priced them is not traced here.

### Speed

r1971, 01:19Z to 01:32Z. Statements a second, median of 5 rounds [range], and the median of the
rounds' p50 latency. Load average 20.5 to 50.1 on 32 cores through the rounds (other lanes' work),
CPU pressure avg10 2.7 to 37.4%, I/O pressure avg10 0.0 to 2.0%; the alternated order gives each
engine the same drift.

| statement | clients | A | B | Turso | SQLite | B/A | B/Turso | B/SQLite |
|---|---|---|---|---|---|---|---|---|
| aggregates_range | 1 | 1,903.5 [1,827.5-2,278.5] p50 0.484 ms | 2,085.5 [1,678.0-2,338.5] p50 0.452 ms | 5,372.0 [5,055.0-5,904.0] p50 0.172 ms | 8,290.5 [6,756.0-8,651.0] p50 0.109 ms | 1.10x | 0.39x | 0.25x |
| aggregates_range | 32 | 3,174.0 [2,278.0-4,642.0] p50 8.128 ms | 2,437.5 [2,173.0-4,767.0] p50 12.8 ms | 26,762.5 [22,141.5-28,888.5] p50 1.0 ms | 4,485.0 [4,127.0-5,287.5] p50 7.104 ms | 0.77x | 0.09x | 0.54x |
| filter_no_index | 1 | 13.0 [11.5-13.5] p50 77.8 ms | 13.5 [11.5-14.5] p50 73.7 ms | 41.0 [36.0-43.0] p50 24.1 ms | 69.5 [42.0-76.5] p50 14.0 ms | 1.04x | 0.33x | 0.19x |
| filter_no_index | 32 | 16.0 [16.0-19.5] p50 2,752 ms | 16.0 [16.0-16.0] p50 2,785 ms | 93.5 [88.0-124.0] p50 324 ms | 20.0 [19.0-34.0] p50 1,475 ms | 1.00x | 0.17x | 0.80x |
| join_three | 1 | 198.0 [165.5-208.5] p50 4.93 ms | 197.0 [171.0-210.5] p50 4.99 ms | 66.0 [61.0-72.0] p50 15.1 ms | 2,458.0 [2,238.0-2,499.0] p50 0.392 ms | 0.99x | 2.98x | 0.08x |
| join_three | 32 | 430.5 [336.0-860.5] p50 76.8 ms | 471.0 [342.0-672.0] p50 68.6 ms | 989.0 [895.0-1,107.0] p50 27.6 ms | 976.0 [907.0-992.5] p50 32.5 ms | 1.09x | 0.48x | 0.48x |
| table_count | 1 | 131.0 [122.0-135.5] p50 7.55 ms | 126.5 [104.5-138.0] p50 7.74 ms | 40,333.5 [39,044.0-41,994.0] p50 0.023 ms | 8,436.0 [7,143.5-8,714.5] p50 0.111 ms | 0.97x | 0.003x | 0.015x |
| table_count | 32 | 1,950.0 [1,863.0-2,067.5] p50 14.3 ms | 1,904.5 [1,612.5-2,145.0] p50 13.8 ms | 722,688.0 [694,499.5-782,101.0] p50 0.030 ms | 2,557.0 [2,217.0-4,227.5] p50 12.0 ms | 0.98x | 0.003x | 0.74x |
| table_group_by | 1 | 9.5 [7.5-10.5] p50 102 ms | 9.5 [8.0-11.0] p50 108 ms | 5.0 [4.5-5.5] p50 203 ms | 7.0 [6.5-8.0] p50 135 ms | 1.00x | 1.90x | 1.36x |
| table_group_by | 32 | 16.0 [16.0-31.5] p50 2,556 ms | 16.0 [16.0-32.0] p50 2,523 ms | 17.5 [15.0-23.0] p50 1,704 ms | 0.0 [0.0-0.0], none answered in the window | 1.00x | 0.91x | none |

- Errors: 0 for every engine, statement and round.
- Answers: for each statement the set of answers over every round and client was identical on A,
  B, Turso and SQLite (`aggregates_range` 32 distinct parameter sets, `filter_no_index` 30,
  `join_three` 32, `table_count` 1, `table_group_by` 1).
- SQLite's `table_group_by` at 32 clients answered no statement inside the 2 s window in any
  round, with no error. It is a statement slower than the window, not a failure.
- Turso's `table_count` (0.023 ms for 200,000 rows) answers the right count far faster than a scan
  of the rows; how is not looked into here.
- Not checked this hour: a profile of any of these runs (RULES 0), so the limiter behind each gap
  is unnamed. The bench workspace's own release profile was used (no LTO, `debug =
  "line-tables-only"`), as R6's script does; the root workspace's profile adds thin LTO and one
  codegen unit.

## 3. Is the column path meant to be on by default?

No, not for every table. Decision 1, item 1 of `docs/design/columnar.md` says a table's columns are
"a copy derived from its rows, declared on the table, as SQL Server's and TiDB's are". What ships is
a process-wide switch, off by default: `AMPHORA_COLUMN_PATH=on` (`column_path::ENV`, read once at
open), and only on the versioned storage (`AMPHORA_STORAGE=versioned`, also off by default). No
per-table declaration exists in `columns_sql.rs`, `columns_path.rs` or `switch.rs` (no match for
"declar"). So the design says declared per table, and the shipped default (off) does not contradict
it; what is missing is the declaration.

## Runs

- r1951 (adacachy2, 00:59Z to 01:15Z): built arm A in 13 min 25 s and arm B in 2 min 2 s, loaded
  Amphora in 6 s, Turso in 3 s, SQLite in 3 s, made the calibration shared, traced the paths.
- r1971 (adacachy2, 01:19Z to 01:32Z, cpus=16 granted at the first ask): the 5 timed rounds, exit 0.
- r1958 (macOS, queued): `cb4_ab_both_rebuy_policies_at_the_extremes` on main 90c1ff8d with
  `--nocapture`, to record the waves scenario's rounds (held, asked, refused). Still queued at the
  end of the hour. Its output answers whether round 2's room is 0 as read above.
