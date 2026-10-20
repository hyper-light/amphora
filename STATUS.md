# Status

Updated 2026-10-20. A piece is done when its tests pass in the gate (`scripts/gates.sh`: Linux, with the
macOS and Windows lints) and the evidence its requirement names is recorded.

## Implemented

- **The bar** (kiln's CP-M1): a Cargo workspace whose lints deny panics, this repository's rules
  (`CLAUDE.md`), the requirement registry (`docs/requirements.toml`), the design's starting point
  (`docs/design/README.md`), the gate (`scripts/gates.sh`, run by kiln CI from `.kiln/ci.yaml`), and the
  vendoring tool (`scripts/vendor.py`, kiln's).
- **The `amphora` binary** says what it is (`amphora --version`, `amphora --help`), and nothing more yet.
- **The store's commit pipeline** (CP-T1.1, `crates/amphora/src/store.rs`, docs/design/store.md): one
  writer with a group commit and no timer; durable beside it by a flusher (a write is durable once a
  flush begun after its commit ends, `Store::durable` by its batch's ticket); readers of their own on
  their caller's thread, each waiting only for its key's writes; no checkpoint in a commit's way (the
  log copied until little is left before the writer starts it again). A full queue refuses rows,
  typed and counted, and `put_wait` waits for room. A commit that fails is retried batch by batch:
  a batch that can't be committed is refused to its sender (`Refused::Failed`), the rest kept. The
  writer keeps no statement journal. Its meter keeps the longest commit, flush, checkpoint, wait to
  restart and restart. Tested in `crates/amphora/tests/store.rs`.
- **Stores on Turso** (`crates/amphora/src/turso.rs`): a database opened for the pipeline, its
  readers, its flusher, its checkpoints and its log's restart.
- **Turso, vendored** (`vendor/turso`, `UPSTREAM.md`): tursodatabase/turso at 3e09a82, the four
  crates Amphora builds (core, sqlite/parser, macros, extensions/core) and a cut-down root manifest,
  with the changes its `UPSTREAM.md` lists. It has no entry in `vendor/MANIFEST.toml` yet:
  `scripts/vendor.py` takes a project's whole tree, and Turso's four crates are a part of it. The
  entry, with `include = ["core", "sqlite/parser", "macros", "extensions/core"]`, comes once
  `vendor.py` takes an include list (kiln's CP-M1).

## Next

- **One thread making everything durable**: on btrfs every flush is a commit of the filesystem's
  log, one at a time, so the checkpointer's flushes of the file (and of the log before each pass)
  hold up the flusher's, and with it every write (docs/design/store.md, decision 4: commits stalled
  up to 97 ms, writes at 512 writers lost a third of their rate). Closing it: the checkpoint copies
  only frames the flusher has made durable (no flush of the log of its own), and the file's flush
  goes to the flusher's thread, between its flushes of the log, before the log starts again. It
  changes when Turso's checkpoint makes what it copied durable, so it lands with T-T1.1.1's crash
  injection, which proves it.
- **CP-T1.1**: kiln vendors Amphora and its conversation store runs on `amphora::store` in place of its one writer thread; kiln vendors Amphora and its conversation store runs on `amphora::store` in place of its one writer thread; the soak bench for drops (AC-T1.1.5), crash injection (T-T1.1.1) and the stalled-disk test (T-T1.1.2); the bar measured on a quiet machine, best of 3 (AC-T1.1.2–4), with vanilla Turso, SQLite and Vitess beside Amphora (`bench/`).
- **CP-T1.2**: Baselines on a quiet machine against SQLite, Postgres 17, Vitess, CockroachDB and TiDB; the research; the client protocol; `docs/design/` written to it.
- **CP-T1.3**: Turso's MVCC with `BEGIN CONCURRENT` made serializable and production-grade, and the engine multi-writer on it.

## Open

Everything else the track's plan names (kiln plan `01a157fdefba746a94a5fcd920a80d5a`): each goal, its criteria and how each is
measured are in `docs/requirements.toml`.
