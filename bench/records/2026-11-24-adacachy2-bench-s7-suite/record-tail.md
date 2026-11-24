## Files in this directory

- `TABLES.md`: the full tables: section 1 (every op, client count and target, with ops/s, p50, p99, p99.9, errors and retries, median and range over rounds), section 2 and 3 (below in this record), section 4 (beside lane S4's table, every target, every cell), and the order check.
- `PHASES.md`: load, CPU pressure and I/O pressure per phase, from `phases.log` (every 5 s). `rounds.txt`: the load, CPU and I/O pressure as each round started and ended. `procs.txt`: the processes using the most CPU when each round started (command lines redacted for anything that looks like a secret).
- `round1.jsonl` .. `round5.jsonl`, `smoke.jsonl`: the raw suite records (`bench/suite.sh` as S4 wrote them), with each record's answers removed (they are read back by `suite_answers.py`, value for value, before the timed runs; the verdicts are `roundN-answers.txt`, `smoke-answers.txt`). The full raw files with answers stay on adacachy2 in `~/.cache/amphora-perf/bench-s7-run/out`.
- `rounds.sh`, `s7_tables.py`, `phases_summary.py`, `smoke_summary.py`, `finish.sh`: the scripts as run. `rounds.sh` ran at sha256 98d1bdae... (checked before the run).
- `bench-cargo-lock.diff`: what `cargo build --offline` added to the untracked `bench/Cargo.lock`. `build.log.tail`: the end of the build's log.

## Not done in this lane

- No cause was looked for: no profile, trace or log of the engines was taken, so every loss here is a measured gap, not an explanation (RULES.md 0; the next lanes' traces own the causes).
- The 35 statements S6's list leaves out (the analytics beyond the five, the text, range and JSON operations, the windowed and set functions) were not measured: see the statement list in `rounds.sh`.
- No single machine-state control: the machine ran other people's work throughout (see "Load that was not this lane's"), so a cell whose ranges of rounds touch is marked "overlap", and only cells whose ranges don't touch are called ahead or behind.
