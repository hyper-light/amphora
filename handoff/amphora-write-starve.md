# amphora-write-starve handoff (goal 9c63f7d4, 2026-11-09)

Agent amphora-write-starve (kiln 01a1b8ceff0872f58d03f25b8c28505f). Plan 01a1b8d174dc72d18b9d2289b6e8fe11, research
01a1b8d174e17549916bd3fc88081ff9. Nothing pushed, nothing merged, nothing built for merge before Ada's word.

## Branch

- Worktree ~/Projects/amphora-t1-write-starve, branch t1/write-starve, from 0668f39 (main f99b758 + t1/floor +
  t1/guards-flake).
- Head 044c44f, tree 286ee8eea4c986e1944a696afb9eb5ccee61b582. Commits: 9416f2d research, 7713063 + 20b863a read-set
  recorder and traces, 8771126 model, ec337ba prototypes + driven checks, bad0a39 driven record, 4b8c510 env switch,
  858a0a2 + 044c44f decision draft and kiln-leg record.
- Code: crates/engine/storage/pager.rs (`Pager::record_access`/`take_access`/`end_access`, `Access`),
  crates/amphora/src/db.rs (`prepare_traced`, `page_image`, `root_page`), crates/amphora/src/replication/admission.rs
  (the pure core: `Admission`, `Window`, `only_size`), crates/amphora/src/replication.rs (`Prepared.reads`, the window,
  `admit`), crates/amphora/src/database/replicated.rs (the slot: `SLOT`, `hold`, `hold_for`, `deferred`, the tail rule
  under a slot admission, answer at apply under a page admission; tests: delay-capable Pump, list-append isolation run).
- Gates run: fmt, clippy -p amphora --all-targets, check-production, cargo test -p amphora on studio local-2 at ec337ba
  (all pass); at 044c44f fmt, clippy, check-production and cargo test -p amphora --lib (148 passed, 3 ignored) pass. Not run: Linux and Windows clippy, the workspace test, vendor.py check
  (the full gate is for whatever Ada chooses).

## AMPHORA_ADMISSION_PROTOTYPE

Read once at `Machine::open`; unset is today's rule. A value it doesn't know is refused, typed.

| value | candidate | rule |
|---|---|---|
| `exact` (or unset) | A0, today | a write applies only on the entry before it |
| `pages` | A1m | applies if no entry since its base (within 4,096) wrote a page it read or wrote; page 1's size change conflicts only with another allocation |
| `pages-then-slot` | D | `pages`, and the leader holds its own writes for a member refused twice in a row |
| `exact-then-slot` | G | `exact`, and the leader holds its own writes for a member refused once |
| `slot` | C | the leader holds its own writes from a member's ReadIndex |

Each slot ends when that member's write applies, or after `SLOT` (200 ms). Under a slot admission a member prepares at
its log tail. Every member of a group must run the same value.

## The real-process test (kiln's promotion test)

The e2e harness to reuse: the scratch kiln copy, ~/.cache/write-starve-kiln on adacachy (a git repo, commit 7ad48c5):
kiln p2p/stall 3aa1cb35097 by `git archive` (kiln's own worktree untouched), its vendor/amphora (f99b758 + kiln's five
patches) carrying t1/write-starve 9416f2d..4b8c510, with the floor session's tail rule ported (that copy predates the
floor). Synced to studio local-2 as ~/.kiln/reach/451397fcc585f7f7cb9aaf00e42064bb/write-starve-kiln-65296113ea (its
first sync came short; `git checkout -- .` there restored it). Its build target: ~/.cache/write-starve-kiln-target on
local-2. Remove both copies and the target once you're done with them.

The run script, ~/.cache/write-starve/kiln-leg.sh on local-2 (`kiln-leg.sh <admission> <label>`): exits if the heavy
lock is held; copies the runners' logs out each second (the test's tempdir goes at its end); prints the test's gap
lines and each runner's `write_again` maximum and unknown outcomes. The command it runs, from the kiln copy's root:

```
export PATH=$HOME/.cargo/bin:$PATH CARGO_TARGET_DIR=$HOME/.cache/write-starve-kiln-target CARGO_TERM_COLOR=never \
  RUST_BACKTRACE=1 CARGO_PROFILE_DEV_DEBUG=line-tables-only CARGO_INCREMENTAL=0 TMPDIR=<a dir of its own>
AMPHORA_ADMISSION_PROTOTYPE=<value> nice -n 10 cargo test --locked -j 8 -p kiln --test p2p_group -- \
  --test-threads=1 --exact p2p_a_promotion_to_voter_costs_the_groups_writes_no_gap_past_an_election_timeout --nocapture
```

Results so far (debug, local-2 at load 3 to 24): bench/records/2026-11-09-write-starve-kiln-leg.md. Runs and logs on
local-2 under ~/.cache/write-starve/kiln/runs/<label>/.

## The release benches (yours, on Ada's word)

What I would run on studio-2 under its heavy lock, best of N (CLAUDE.md §7):
1. The kiln promotion test in release for `exact`, `exact-then-slot`, `pages-then-slot` (add `--release` to the cargo
   test above, a target dir of its own), N=5 each, recording each runner's `write_again`, unknowns, promotions, and the
   test's gap lines; the writer's commits and not-committed lines (`election timeout ...; N commits, M writes not
   committed`) give the leader's cost.
2. The model is simulated time and needs no quiet host: `AMPHORA_TRACES=<uncompressed traces>
   AMPHORA_MODEL_OUT=<out> cargo test --release --locked -p amphora --test write_starve_model -- --ignored the_ab_sweep`;
   traces: `zcat bench/records/2026-11-07-studio-local2-write-starve-traces.jsonl.gz`; tables: `python3
   bench/write_starve_table.py <out>`.
3. The driven isolation run: `cargo test --locked -p amphora --lib every_prototype_admission -- --nocapture`.

## Open

- Step 5's release benchmarks (above) and step 6's seam-owner review: the notice is
  ~/.cache/claude-scratch/amphora/codex-notices/t1-write-starve.md, sent under key write-starve-decision-2026-11-09.
- kiln step 10's own 200 ms gap assertion fails under every candidate; gaps of 0.45 s to 1.71 s appear before any
  promotion. That is kiln step 10's question.
- Not built: B's wire path (a command entry, a leader-side queue, the result's way back), E/EC (row rebase), F.
- A replicated-class follower under a page admission answers at its apply, not on the commit (`verdict` knows only the
  exact rule).
