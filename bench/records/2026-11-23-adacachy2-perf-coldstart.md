# perf/coldstart, AUDIT §7 item 1: the open's scratch page read, once per IO backend and device

Host: adacachy2 (Linux x86_64, 32 cores, NVMe, kernel 7.3.3-1-cachyos), shared with other builds
(3 to 25 of 32 cores idle during the runs: the noise below is that load). Release profile.
Before: 39aa5763 (origin/main b817508b + the example). After: 31720420.

Command (kiln run r94, one machine, before and after interleaved, 5 reps x 200 opens each):

    sh .perf/open_time.sh 39aa5763 200
    # = cargo build --release -p amphora-engine --example open_time; open_time <dir> 200

`open_time` opens 200 new database files one after another, each with a new PlatformIO as
crates/amphora/src/db.rs does, then reopens each; it times `Database::open` and the first statement.

| phase | before min / median of 5 medians (us) | after min / median of 5 medians (us) |
|---|---|---|
| new file open | 233.7 / 347.6 | 189.9 / 278.8 |
| reopen | 173.4 / 259.1 | 144.7 / 160.4 |
| first statement on a new file (CREATE TABLE, fsync-bound) | 7359 / 18657 | 7791 / 19288 (no change; noise) |
| reopen's first statement | 8.6 / 11.7 | 7.4 / 8.3 |

Per-rep medians, new open: before 345.1 316.1 351.5 382.0 347.6; after 347.1 284.1 258.7 273.8 278.8.
Per-rep medians, reopen: before 305.5 291.8 183.2 259.1 191.1; after 248.4 156.2 156.2 160.4 160.5.

Reading: the scratch page (create, write 4 KiB, timed reads, unlink) cost about 30-40 us of every
writable open after the first on a device; the min falls 18% (new) and 17% (reopen). The rest of an
open (~145-210 us) is elsewhere: perf-D's bootstrap/fdatasync and perf-C's versioned open are the
next pieces of it.

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
