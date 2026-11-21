# amphora-record-f4: kiln goal d38fd52c step 3, the finding-4 bench record (2026-11-12)

- Branch t1/record-f4 (from batch head b151af0), one commit, signed through kiln (x509, kiln-agent; `%G?` E
  locally, as every commit before it).
  head a3d7eb59e5aaa50f3e2f3986cbae4a403299ba68, tree 32f5dc59253d460c692a044a8557461fef456848.
- File: bench/records/2026-11-12-finding4-hot-loop.md (record only; no code changed, no tests apply).
- Source: the finished kiln background run of f4.sh (`CALM_LOAD=1000 NO_BUILD_CHECK=1 bash ./f4.sh`) on runner
  cec37f60 (label adas-mac-studio.local, hostname Adas-Mac-Studio-3.local; M4 Max, 14 cores, 36 GiB, macOS 26.2),
  2026-11-12 03:52:07Z-03:56:46Z. Outputs read in place in ~/.cache/finding4-bench; nothing removed, nothing rerun.
- Before a005407 (tree da22e42b), after 839cd0e (tree 512c0f4a); binaries eeae29de77e498a2 / 402df2503d0b9af4.

Best of 3, ns a statement, wall time (inside / whole):
| shape | a005407 | 839cd0e | inside change |
| reapi | 3,550.2 / 3,565.7 | 1,873.3 / 1,890.3 | -47.2% |
| logsearch | 40,186.8 / 40,461.7 | 21,711.2 / 21,982.9 | -46.0% (checksum 3e1f7c81cd4bf7f5 both) |
| insert | 3,304.9 / 8,520.4 | 2,196.7 / 7,340.4 | -33.5% (whole -13.8%) |
Parse control (same code at both): moved at most 1.7%.
Profile parser share (prof_share.py): a005407 reapi 0.09%, logsearch 1.80%, insert 0.68%; 839cd0e 0 for all.

Missing, said in the record:
- Load over CLAUDE.md's bar: 1-min 2.42-3.72, 5-min 3.11-3.37; the gate was off on Ada's word (2026-11-11).
- No Linux leg (the step asked for fedora or jupiter); no CPU time (schedstat is Linux's).
- The macOS profile doesn't place the ~47% saving (parser 0.09-1.8% of samples before). Its attribution is weak:
  the writer thread unnamed, insert's sample count ~165 against ~860 expected, and 47-63% of insert's samples in the
  bench's own failed schedstat open. A Linux perf run would settle where the cost went.

## Linux leg (Ada, 2026-11-12: "Owe a Linux leg"): launched, waiting
- Record merged into the batch at e283424 (Amphora agent). Step 3 waits for this leg.
- Carry repo ~/.cache/claude-scratch/finding4-carry: e4d550d commits f4.sh as it ran on macOS (the CALM_LOAD /
  NO_BUILD_CHECK change was uncommitted; the record cites d2bb127, fix that in the Linux section), 6b04984 adds
  f4-linux.sh (sha256 9d91a46f30b61c66).
- adacachy2 (00aad8da): systemd user unit amphora-record-f4-linux-20261112T074654Z (RuntimeMaxSec 25 h,
  RUSTUP_TOOLCHAIN=1.99.1), started 07:46:54Z, running from ~/.cache/finding4-bench-linux/carry. It looks for the
  heavy lock every minute (24 h at most; held by the planner batch since 06:47:32Z), frees it on every exit.
- perf is not installed on adacachy2 (nothing installed): the profile is the bench's own (pprof) + prof_share.py.
- Host: AMD Ryzen 9 9950X3D2, 32 threads, 59 GiB, Linux 7.3.3-1-cachyos, governor powersave.
- Read: `cat ~/.cache/finding4-bench-linux/log` and `systemctl --user status amphora-record-f4-linux-20261112T074654Z`.
