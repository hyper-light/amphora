# perf/coldstart, AUDIT §7 item 1: the open's scratch page read, once per IO backend and device

Host: adacachy2 (Linux x86_64, 32 cores, NVMe, kernel 7.3.3-1-cachyos), shared with other builds
(3 to 25 of 32 cores idle during the runs: the noise below is that load). Release profile.
Before: 39aa5763 (origin/main b817508b + the example). After: 31720420.

Command (kiln run r94, one machine, before and after interleaved, 5 reps x 200 opens each):

    sh .perf/open_time.sh 39aa5763 200
    # = cargo build --release -p amphora-engine --example open_time; open_time <dir> 200

`open_time` opens 200 new database files one after another, each with a new PlatformIO as
crates/amphora/src/db.rs does, then reopens each; it times `Database::open` and the first statement.

| phase | before best / median of 5 run medians (µs) | after best / median of 5 run medians (µs) |
|---|---|---|
| new file open | 316.1 / 347.6 | 258.7 / 278.8 |
| reopen | 183.2 / 259.1 | 156.2 / 160.4 |
| first statement on a new file (CREATE TABLE, fsync-bound) | — / 18657 | — / 19288 |
| reopen's first statement | — / 11.7 | — / 8.3 |

Per-rep medians, new open: before 345.1 316.1 351.5 382.0 347.6; after 347.1 284.1 258.7 273.8 278.8.
Per-rep medians, reopen: before 305.5 291.8 183.2 259.1 191.1; after 248.4 156.2 156.2 160.4 160.5.

Across these runs the best median fell 18.2% for new opens and 14.7% for reopens.
The record preserves only the aggregate medians for first statements. Other builds shared
the host, so these observations do not establish an isolated cost for the scratch read (T-QT.5.5).

The original example reused `first.db` and the numbered `open-<i>.db` files in the supplied
directory. Repeating a path made an existing database count toward the new-file samples.
The record does not retain the per-run fixture paths. The revised example creates a private
directory on the requested device for each invocation and requires at least one measured open.

The process's first open (CPU calibration, `process_calibration`, still on the first open) ranged
2.2 ms to 1.0 s across runs on this loaded host (calibration samples until each median settles,
bounded by a page of keys): that is the remaining §7.1 work (cache per machine across processes).

Planner test A/B (kiln run r89, same host, under load): `tests/planner.rs`
`a_whole_table_group_by_scans_and_sorts_where_lookups_would_read_pages` failed 2 of 3 runs on
before and 3 of 3 on after: a load-dependent flake on main itself, not settled by this change.

## AUDIT §7.1, second part (perf-H2): CPU calibration kept per machine across processes

Host: adacachy2 (as above), shared with other builds (18 of 32 cores idle at the start). Release
profile. Before: origin/main dcd6ce4c. After: the synced tree (perf/coldstart, this commit).

Command (kiln run r133, one machine, before and after interleaved, 25 fresh processes each, one
shared empty `XDG_CACHE_HOME` for the run):

    sh .perf/first_open.sh dcd6ce4c 25
    # each process: open_time <dir> 1, its "process's first open" line (Database::open, the
    # process's CPU calibration included)

| process's first open (us) | min | median | max |
|---|---|---|---|
| before (calibrates every process) | 7567 | 82604 | 161025 |
| after, the machine's first process (calibrates, keeps it) | 91186 | | |
| after, the 24 processes after it (read the kept calibration) | 670 | ~870 | 1257 |

The kept file is 2.4 KB (`$XDG_CACHE_HOME/amphora/calibration/<fnv of cpu + exe path>`). A process
reads it back bit for bit (the times as f64 bits), so it plans on exactly the rates the
calibration measured; another CPU record (model, stepping, microcode, flags, bugs) or another
executable (path, length, mtime) is another key and is measured again. Linux only: macOS and
Windows name their CPU through calls this crate doesn't make (no unsafe), so they calibrate each
process, as before.

## AUDIT §7.6, first half (perf-H3 code, perf-H4 run): a compacted log's member opens without its image

PRELIMINARY, not a verdict. One partial run of `GIBS="1 10" sh .perf/u1_bench.sh` on adacachy2, started before
RULES rule 6's revision (03:35Z): no `needs`, load average 69-89 at its start and 81-130 at its end from other
lanes, built from the synced clone (perf/coldstart 4117e676 with a code motion of `Machine::image` since reverted;
the image path did the same work). The 10 GiB arm was stopped during its fill. Rerun under rule 6 (cpus=16, both
arms from fixed SHAs in worktrees under ~/.cache/coldstart-ab, load beside every run) before quoting a number.

`image` = `Machine::open` then the whole image made in memory (what `Replica::open` did for a compacted log);
`count` = `Machine::open` then `image_bytes` (what it does now). 1 GiB database (1,082,327,040 bytes), page cache
warm, alternated, 5 runs a side:

| side | open_ms runs | median | image step (open_ms - machine_open_ms) | peak RSS |
|---|---|---|---|---|
| image (before) | 1857.4 2012.6 2155.8 1865.1 1487.3 | 1865.1 | 930.0 904.8 890.3 892.0 910.5 (median 904.8) | 3,245-3,246 MiB |
| count (after) | 740.8 1577.3 1177.1 1650.9 1356.8 | 1356.8 | 0 | 69-70 MiB |

Both sides state image_bytes=1083631733 (the count equals the bytes `image` wrote). The image step is a steady
~0.9 s a GiB (about 1.2 GB/s, page cache warm) and 3x the image in memory (pages, their encoding, the copy).
`Machine::open` itself took 577-1651 ms under the host's load and dominates the total's spread, so the total's
gap (median 1865 -> 1357 ms) is inside that noise; the image step and the memory are the measured change.
