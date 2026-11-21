# amphora-benches handoff (agent 01a1c0fb814273389cbb657f543cb382), 2026-11-09 ~16:50Z
Stopped on the Amphora agent's word (Ada: agents implement and unit-test; the Amphora agent owns the benches).
No runs left: r5 (studio-3), r11 and r19 (cec37f60) stopped, both hosts' heavy locks free, no bench processes.

## 60805ec4 part B: t1/wal-small-commits
- Worktree ~/Projects/amphora-t1-wal-redo, branch t1/wal-small-commits, head 3e91d70 (tree 5f1a0e5b2b693c890561fa6c0db024989108ee4a),
  clean: main db991a0 merged into 4acb85d. Conflicts: docs/design/store.md (main's 34/36 beside the branch's 35/37, in
  number order), bench/Cargo.toml (main's comment on [patch.crates-io]), crates/amphora/tests/crash.rs (the same
  as_chunks fix on both sides, kept once).
- Tests on adas-mac-studio.local-2 (debug, -j 8): `cargo test --locked --no-fail-fast -p amphora -p amphora-engine`
  all green at 3e91d70 (amphora lib 153 passed/2 ignored; amplification 2, crash 5, every engine suite incl. packed_log
  12, shared_pages 9, reader_cache 6, spilled_log 2; engine lib 2691). The seven gates were not run (the batch rule).
- Left: step 2's quiet-Mac bench record (decision 37's bound on the batched leg; the 1/4/8 MiB legs and the suite's bulk
  statements no slower than 13e385d), then the record committed on top and the head gated.
- Bench script: ~/.cache/claude-scratch/wal-redo-carry/bench-l2.sh (its git repo, head 4c34288). Command, as a kiln
  background run with cwd = that carry dir, on one quiet Studio:
    bash ./bench-l2.sh 09ceb0ab4d6b682a759ecad817eca1b7fd1644ae d42fbd9ea5b41e03606f9fd85721c2cab1fa5d57 f32571349438798416d54a12f5a2f84de0443198 4acb85d
  The trees: step2 = 4acb85d (09ceb0ab...), step1 = da367e6 (d42fbd9e...), base = 13e385d (f3257134...), each made from
  base.tar.gz plus step{1,2}-from-base.patch, checked by tree hash, plus the audit's zz-audit harness and corpus
  (harness/). It takes the heavy lock, builds zz-audit for all three and amphora-bench for step2 and base, then two
  rounds (step2, step1, base) of `zz-audit legs <dir> <corpus> 120 1 4 8` and `zz-audit batched <dir> <corpus> 1`, each
  begun after 90 s at load < 2, then the suite's writes (--suite --items 200000 --clients 1,32 --seconds 5 --only
  insert_one,...,txn_read_write) for step2 and base. Env: NO_SUITE=1, NO_LEGS=1, ROUNDS, VARIANTS.
  Outputs: ~/.cache/wal-redo-bench-l2.log (+ .build, .suite), ~/.cache/wal-redo-bench-l2-suite-{step2,base}.jsonl.
- Trees and release binaries are already built on studio-3 (2768d0d5) AND on cec37f60 (adas-mac-studio.local, the
  fourth Studio) under ~/.cache/wal-redo-bench-l2*: a rerun there skips the builds (zz-audit step2 46eac942..., step1
  beaef016..., base 07fa057f...; amphora-bench step2 ea68e8e0..., base 542b63ec... on studio-3).
- Partial, NOT a record (one round, load ~1.5-3): studio-3 2026-11-09, step2 legs 1/4/8 MiB took 1.26/6.95/16.21 s (the
  11-07 record had 13e385d at 1.24/6.99/15.20); step2's batched leg: disks wrote 237 MiB for its MiB of text (bound 481;
  step1 wrote 427-442). Note: zz-audit's own line said "wrote 0 MiB to storage (0x log) ... wal 24 MiB": its storage
  counter doesn't see a mixed log's writes; the disks' counter (ioreg) is the one the record uses.
- Hosts: studio-3's load sat at 2.2-2.9 from a kiln-ui-bench Docker container (kiln-ui-bench-p5s7, ~100% CPU, created
  16:20Z); studio-2 is 99% full (5 GB free). I removed two stale amphora-wal-redo heavy locks (studio-3, owner
  2026-11-08T03:12:30Z; cec37f60, owner 2026-11-08T01:47:45Z), each after checking no process of its run was left.

## d38fd52c step 3: finding 4's hot-loop bench
- Before = a005407 (tree da22e42b76ed6d7dd8d07cf30fa22f8a705cac0f): the bench and its profiler, before the fix
  (5a70168's parent). 839cd0e's parent is 0adb89b, which already carries the fix, so it can't be "before".
  After = 839cd0e (tree 512c0f4a2dde69279d859f564f4786cccd31740e). bench/ is the same at both.
- Script: ~/.cache/claude-scratch/finding4-carry/f4.sh (its git repo; a005407.tar.gz, 839cd0e.tar.gz from git archive).
  Command, as a kiln background run with cwd = that carry dir: `bash ./f4.sh`. It takes the heavy lock, unpacks and
  checks both trees, builds amphora-bench in release, then per commit (each begun after 90 s at load < 2):
    amphora-bench --sql-api --dir <db> --shapes reapi,logsearch,insert,parse --runs 3 --label <commit>
  (defaults: 100,000 digests, 10,000 pages, 100,000 rows), then one profiled run each:
    AMPHORA_PROFILE=<prof>/<commit> amphora-bench --sql-api --dir <db> --shapes reapi,logsearch,insert --runs 1 --label <commit>-profiled
    python3 <tree>/bench/prof_share.py <prof>/<commit>-*-1.folded
  Outputs: ~/.cache/finding4-bench/{log, log.build, <c>-timed.jsonl, <c>-profiled.jsonl, <c>-share.jsonl, prof/}.
  Trees and binaries are built on cec37f60 (amphora-bench a005407 eeae29de..., 839cd0e 402df250...): a rerun skips builds.
- Partial, NOT a record (cec37f60, load 1.4 one-minute but 3.9 five-minute): a005407 best of 3 per execute: reapi
  4,085 ns, logsearch 39,492 ns (checksum 3e1f7c81cd4bf7f5), insert 11,370 ns; parse of the page query 13,988 ns.
  On macOS the CPU-time columns are null (schedstat is Linux-only): wall time only.
- Not started: t1/finding4-bench branch (nothing to commit without a record).

## Findings
a) main db991a0's bench/Cargo.toml has [patch.crates-io] twice (lines 39 and 55, from merge 2472173): `cargo check` in
   bench/ fails with a duplicate key, so main's amphora-bench doesn't build; bench_workspace.rs passes (its parser
   accepts both). The bench trees above predate it and build.
b) amphora lib test database::guards::tests::a_child_sharing_the_lease_holds_it_past_this_processs_drain failed once on
   local-2 at 3e91d70 (load ~4-5, full `-p amphora` run), then passed 3/3 alone and in the full rerun. Failing output
   not captured.
