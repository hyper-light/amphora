# perf/snapshots (lane R2): 2026-11-23

Ada's goal: replication holds at agentic Meta scale. A lagging member catches up without the whole
database in memory, without fencing or stalling a healthy member, and without pushing serving p99.9
out. Design: docs/design/replication.md, "Decision (lane snapshots)". Ada's order (2026-11-23): build
the push (one view held for the whole transfer) and the pull (a view per answer) behind one seam and
decide on an A/B.

## The held-view premise (the A/B's first piece)

Test: `crates/amphora/tests/held_view.rs`, `snapshots_a_view_held_across_writes_stops_the_checkpoint_and_grows_the_log`.
Counted, not timed: bytes and frames of log, so it has no run-to-run noise.

Command (adacachy2, debug build, from the lane's GitHub clone):
`cargo test -p amphora --test held_view --locked -- --nocapture --test-threads=8`

No numbers yet. The lane's runs on adacachy2 (r334, r353, r366, r383, all stopped by the lane) spent
their time in `git clone` and `git checkout` blocked in btrfs `wait_reserve_ticket` (metadata 96.8%,
1 MiB unallocated): 2,517 of 3,531 files checked out in 5 minutes, 3,144 in 16. At 05:09Z the
coordinator paused measurement runs there (RULES.md 5d: load 379, 384 threads blocked on disk). The
test counts frames and bytes, not time, so the stall can't change its numbers; it runs once the
pause lifts.

What the premise's serving half looks like in the code (not measured): past the log's room the owner
holds back every new read of the replica until a restart (`database/replicated.rs`, `hold_due`,
`hold_reads`), the engine refuses the restart as busy while any view is open (`Db::restart_log`), and
the owner refuses submits as unavailable (`log_full`).

## Checks

One run at a time, in the lane's one target dir, with `CARGO_INCREMENTAL=0` for fewer files on the
stalled disk.

r405 (adacachy2, needs linux/amd64,cpus=8,mem=24g): `cargo clippy -p amphora --all-targets --locked
-- -D warnings`; `cargo test -p amphora --lib -- memory:: replication::image_room_tests snapshots_
a_member_installed_from_a_snapshot compaction`; `cargo test -p amphora --test design_index`. Results:
PENDING when the lane's hour ended.

### Lane R3 (05:19Z to 06:14Z)

r405 (R2's checks at af4f3131) printed nothing in its first 13 minutes of R3's hour and was stopped at 05:32Z so
the lane's one target dir could check the newer head.

r417 (adacachy2, needs linux/amd64,cpus=8,mem=24g, `CARGO_INCREMENTAL=0`, the lane's target dir, from the GitHub
clone `~/.cache/amphora-perf/snapshots-remote` at faebe9ba: decisions 1 and 2, the owner-level test, origin/main
78fa9fdb merged): `cargo clippy -p amphora --all-targets --locked -- -D warnings`; `cargo test -p amphora --locked
--lib -- --test-threads=8 memory:: image_room_tests snapshots_ compaction joiner`; `cargo test -p amphora --locked
--test design_index`; `cargo test -p amphora --locked --test held_view -- --nocapture`. Clippy began 05:34:27Z at
load 130.45 157.54 190.50 and was still running at 05:57Z. Results: PENDING when the lane's hour ended; the
held-view counts land here and in replication.md, "The held view, measured", from r417's output.

Not covered by r417 (committed after it checked out): 938e15a0 (the waiters count of waits not yet woken),
d05e55eb (unit B's source, `replication/source.rs`, and its test), 1b906a09 (design text).
