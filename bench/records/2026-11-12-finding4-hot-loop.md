# Finding 4's hot loop through the public SQL API: a005407 against 839cd0e (2026-11-12)

Kiln goal d38fd52c, step 3: the unify audit's finding 4, the per-statement cost of kiln's hot
statements run again and again through `amphora::sql` (`prepare_cached` inside `Database::call`),
before and after the fix, best of 3. Written from the run that finished at 03:56:46Z; nothing was run
again for this record.

## Not to the bar

- **Load.** CLAUDE.md section 7 asks for a load average under 2. This run's was over it throughout:
  the one-minute load was 2.42 to 3.72 and the five-minute 3.11 to 3.37 (the table below). The
  script's load gate was turned off on Ada's word (2026-11-11: "Just run the fucking bench christ"):
  `CALM_LOAD=1000` makes its wait pass whatever the load, and `NO_BUILD_CHECK=1` skips its check that
  nothing is compiling. The wait still took its three looks 30 s apart, so each phase began 60 s after
  the one before.
- **Host.** The step asked for fedora or jupiter. This is a macOS Mac Studio's record; a Linux leg was
  not run.
- **Wall time only.** The bench's CPU-time columns (`inside_cpu_ns_per_execute`, `cpu_ns_per_parse`)
  read `/proc/thread-self/schedstat`, which is Linux's: on macOS they are null in every row, and so is
  the rows' `load`. The numbers here are wall time.

## The run

- host: kiln runner cec37f60f22c5a60d27f08f4d2e5daa8 (label adas-mac-studio.local; `hostname` said
  Adas-Mac-Studio-3.local), Mac Studio, Apple M4 Max, 14 cores, 36 GiB, macOS 26.2, rustc 1.99.1
  (58cf8f9c3 2026-10-13). The script logged these at its start.
- date: 2026-11-12, 03:52:07Z to 03:56:46Z, as a kiln background run, under the host's heavy lock
  (taken at 03:52:07Z, freed by the script's trap; it was gone when the outputs were read).
- before: a005407, tree da22e42b76ed6d7dd8d07cf30fa22f8a705cac0f: the bench and its profiler, before
  the fix (5a70168's parent). 839cd0e's parent, 0adb89b, already carries the fix (5a70168, 86350b6,
  0adb89b), so it can't be "before".
- after: 839cd0e, tree 512c0f4a2dde69279d859f564f4786cccd31740e: the fix and decision 36's mark
  (docs/design/store.md, decisions 34 and 36). `bench/` is the same at both, so the bench's own code
  doesn't differ between the two.
- trees: each made from `git archive` of its commit and checked against its tree hash by
  `git write-tree` (on 2026-11-09, kept since; the log says "kept" for both at this run).
- binaries: `amphora-bench`, release profile (optimized + debuginfo), sha256 beginning
  eeae29de77e498a2 (a005407) and 402df2503d0b9af4 (839cd0e), the same as built on 2026-11-09 (this
  run's build was a no-op: "Finished ... in 0.42s").
- script: `f4.sh` in the carry repo `~/.cache/claude-scratch/finding4-carry` (commit d2bb127, with the
  two archives), run there as `CALM_LOAD=1000 NO_BUILD_CHECK=1 bash ./f4.sh`.
- outputs, on that host: `~/.cache/finding4-bench/{log, log.build, <commit>-timed.jsonl,
  <commit>-profiled.jsonl, <commit>-share.jsonl, prof/}`.

The commands, per commit `<c>`, each in a fresh database directory `<db>`:

```
cd tree-<c>/bench && CARGO_TARGET_DIR=~/.cache/finding4-bench/target-<c> nice -n 10 cargo build --release -j 8 --bin amphora-bench
amphora-bench --sql-api --dir <db> --shapes reapi,logsearch,insert,parse --runs 3 --label <c>
AMPHORA_PROFILE=prof/<c> amphora-bench --sql-api --dir <db> --shapes reapi,logsearch,insert --runs 1 --label <c>-profiled
python3 tree-<c>/bench/prof_share.py prof/<c>-*-1.folded
```

The bench's defaults: reapi 100,000 digests (three statements each: 300,000 executes), logsearch
10,000 pages, insert 100,000 rows, parse 20,000 parses a statement, 1,000 executes (100 pages) a
`Database::call`. The shapes are kiln's unify branch's statements (`bench/src/sqlapi.rs`): reapi is
memo.rs `missing`'s EXISTS, then cas.rs `object`'s SELECT size and its UPDATE of the last use;
logsearch is logsearch.rs's search page; insert its posting INSERT; parse is what the session's
`validate` did to each of those statements on every execute (a whole parse and a walk of its
tokens), timed alone.

| phase (log) | began | one-, five-, fifteen-minute load at the start | at the end |
|---|---|---|---|
| timed a005407 | 03:53:09Z | 2.57 3.33 3.31 | 2.66 3.31 3.30 |
| timed 839cd0e | 03:54:23Z | 2.50 3.15 3.24 | 2.42 3.11 3.23 |
| profiled a005407 | 03:55:33Z | 3.72 3.37 3.32 | 3.61 3.36 3.31 |
| profiled 839cd0e | 03:56:40Z | 2.68 3.16 3.24 | 2.70 3.16 3.24 |

Two earlier starts of the same script on this host (2026-11-09, 16:32Z and 16:47Z) were ended by a
signal after a005407's timed phase; their results were overwritten by this run's and are not part of
this record.

## Before against after: best of 3, ns a statement

"Inside" is the time inside the calls' closures, the statements alone; "whole" is the calls' whole
time, their commits too. Each column's best is its own minimum over the three runs.

| shape | a005407 inside | 839cd0e inside | change | a005407 whole | 839cd0e whole | change |
|---|---|---|---|---|---|---|
| reapi | 3,550.2 | 1,873.3 | −1,676.9 (−47.2%, 1.90×) | 3,565.7 | 1,890.3 | −1,675.4 (−47.0%, 1.89×) |
| logsearch | 40,186.8 | 21,711.2 | −18,475.6 (−46.0%, 1.85×) | 40,461.7 | 21,982.9 | −18,478.8 (−45.7%, 1.84×) |
| insert | 3,304.9 | 2,196.7 | −1,108.2 (−33.5%, 1.50×) | 8,520.4 | 7,340.4 | −1,180.0 (−13.8%, 1.16×) |

Logsearch's checksum of every page's rows was 3e1f7c81cd4bf7f5 at both commits, in every run: the
two builds gave the same answers.

The runs, inside / whole, ns a statement:

| shape | commit | run 1 | run 2 | run 3 |
|---|---|---|---|---|
| reapi | a005407 | 3,792.9 / 5,361.5 | 3,577.6 / 3,621.6 | 3,550.2 / 3,565.7 |
| reapi | 839cd0e | 2,071.2 / 3,601.8 | 1,907.9 / 1,963.4 | 1,873.3 / 1,890.3 |
| logsearch | a005407 | 40,500.4 / 42,139.6 | 40,186.8 / 40,461.7 | 41,277.6 / 41,569.8 |
| logsearch | 839cd0e | 22,618.4 / 24,354.6 | 21,743.9 / 22,021.2 | 21,711.2 / 21,982.9 |
| insert | a005407 | 3,325.6 / 8,520.4 | 3,304.9 / 8,539.9 | 3,310.0 / 8,969.5 |
| insert | 839cd0e | 2,236.9 / 7,340.4 | 2,196.7 / 7,439.4 | 2,273.2 / 7,478.9 |

Each commit's first reapi run is slower than its other two in whole time (5,361 and 3,602 against
about 3,600 and 1,900); the bench doesn't warm up before run 1, and this record doesn't say why.

### The parse shape: the control

The parse shape calls the parser directly, in bench code that is the same at both commits, so the fix
can't move it: it shows how far this run's noise goes. Best of 3, ns a parse:

| statement | a005407 | 839cd0e | change |
|---|---|---|---|
| exists (reapi) | 1,738.5 | 1,709.5 | −1.7% |
| size (reapi) | 609.3 | 605.2 | −0.7% |
| touch (reapi) | 489.6 | 485.5 | −0.8% |
| page (logsearch) | 14,719.0 | 14,507.8 | −1.4% |
| posting (insert) | 585.3 | 587.8 | +0.4% |

The control moved at most 1.7%; the hot shapes' inside times moved 33.5% to 47.2%.

For scale: reapi's three statements parse in 945.8 ns on average at a005407 and its inside time fell
1,676.9 ns; the page parses in 14,719.0 ns and logsearch fell 18,475.6 ns; the posting parses in
585.3 ns and insert fell 1,108.2 ns. Each shape fell by more than one parse of its statements. The fix
also changed what an execute does besides the parse (86350b6: a Statement holds its prepared
statement, the cache's recency is a linked list); this run doesn't separate those parts, and the
profile below doesn't either.

## Where the profile put the cost

One profiled run each (AMPHORA_PROFILE: pprof's sampler, 997 samples a second), its own timings close
to the timed runs' (a005407 inside: reapi 3,833.1, logsearch 42,852.5, insert 3,378.8; 839cd0e:
2,089.2, 23,035.7, 2,343.5 ns).

`prof_share.py`: the share of each shape's loop (samples whose stack holds `sqlapi::<shape>_run`) with
a frame in the SQL parser (`amphora_engine_parser::`) or the session's validation
(`amphora::sql::validate`):

| shape | a005407 parser / loop samples | share | 839cd0e parser / loop samples | share |
|---|---|---|---|---|
| reapi | 1 / 1,056 | 0.09% | 0 / 533 | 0% |
| logsearch | 7 / 388 | 1.80% | 0 / 216 | 0% |
| insert | 1 / 148 | 0.68% | 0 / 126 | 0% |

The fix took the parser out of the loop: no sample at 839cd0e has a parser frame. But at a005407 the
profile put 0.09% to 1.80% of the loop in the parser, while the timed runs lost 33.5% to 47.2% of the
inside time with the fix. On this host the profile does not account for where the saved time was.

Where it did put the samples (`prof/<c>-<shape>-1.txt`, inclusive share of all samples):

- reapi: at both commits about 95-98% of samples under the first statement's (EXISTS) index seek
  (`query_row<bool>` → `op_seek` → `indexbtree_seek_internal` → `read_page` → `begin_read_disk_page`):
  96.27% at a005407, 94.76% at 839cd0e. `validate` 0.09% at a005407.
- logsearch: under page reads (`read_page` 90.79% at a005407, 92.69% at 839cd0e), split between the
  rowid seeks of the joins (`op_seek_rowid` 62.15%, 73.52%) and index seeks (`op_seek` 26.60%,
  15.53%). At a005407 `validate` 1.28%, `validate_reserved_command`'s token walk 1.02%, the parser's
  `parse_select` 0.77%; at 839cd0e none.
- insert: 47.27% (a005407) and 62.69% (839cd0e) of samples are under the bench's own `cpu_since` →
  `read_to_string` → `File::open`, its try at the Linux-only schedstat file; the statement's own work
  (`step`) is 41.82% and 32.09%, most of it page allocation and balancing (`do_allocate_page` 34.55%,
  24.63%). The bench tries that file (`cpu_ns`) twice a `Database::call`, which is a thousand
  executes.

The profile is weak evidence on this host, and these are the reasons. The writer thread wasn't named
in the samples ("writer: samples 0"). The leaf frames are mostly an unnamed `{closure#0}` (98.51% of
reapi's at a005407). The insert leaves are `_os_cpu_copy_in_cksum` and `_open$NOCANCEL`. The insert
run's sample counts are well below 997 a second over its time: 165 samples in all at a005407, against
about 860 for its 0.86 s. The inclusive shares above are what the sampler recorded, and this record
claims nothing past them. A Linux run (perf, with CPU time) would settle where the cost is.

## Missing

- A Linux leg (fedora or jupiter, as the step asked): not run.
- CPU time: none on macOS.
- A run under load 2: none. This run's one-minute load was 2.42 to 3.72.
- The source of the saving: the timings show it, but the profile doesn't place it.
