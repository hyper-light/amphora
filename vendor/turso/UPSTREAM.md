# Turso Database, vendored

The engine of kiln's conversation store (`docs/conversations.md`).

- **Upstream:** https://github.com/tursodatabase/turso, commit `3e09a82750d8290ef96e3830896e7205b7c0b9e6`
  (2026-10-18, version 0.9.0-pre.1), MIT (`LICENSE.md`, `NOTICE.md`).
- **What is here:** the four crates kiln builds, laid out as upstream lays them out:
  - `core` (turso_core)
  - `sqlite/parser` (turso_parser)
  - `macros` (turso_macros)
  - `extensions/core` (turso_ext)
- **How kiln builds it:** `turso_core` with `default-features = false` and
  `features = ["fs", "json", "pure-rust-crypto"]`. That means no `simd` (simsimd is C), pure-Rust
  crypto (no libaegis C), and no `fts` (zstd-sys is C and fails on musl). No C is compiled.
- **Its own workspace:** `Cargo.toml` here is upstream's root manifest cut down to these four members,
  their `[workspace.dependencies]` and `[workspace.lints]`. kiln's workspace excludes `vendor/turso`,
  so kiln's lints don't apply to it, and the crates' `workspace = true` keys resolve here.

## Changes from upstream

| File | Change | Why |
|---|---|---|
| `Cargo.toml` | Cut down to the four crates above | only what kiln builds |
| `core/build.rs` | Never runs `git` (as with `SOURCE_DATE_EPOCH`) | vendored, it would embed kiln's commit, and builds stay reproducible |
| `core/alloc/collections/iterator.rs`, `iterator/nightly.rs` | The `nightly`-only specialization moved into a module of its own, unchanged | inline, stable Rust parses its `default fn` before the cfg drops it, and warns that the syntax will become a hard error |
| `core/io/windows_lock.rs` | `#![allow(dead_code)]` | its shared-WAL locks are unused in kiln's build, so a Windows build warned 17 times |
| `core/storage/btree.rs`, `encryption.rs`, `pager.rs`, `mvcc/persistent_storage/logical_log.rs`, `mvcc/database/mod.rs`, `types.rs`, `vector/operations/text.rs` | `#[allow]`s for what kiln's build leaves unused, `fetch_update` (deprecated on the newest Rust, its replacement missing on older ones) and the aggregate's callback comparison; an elided lifetime spelled out | its build prints no warnings |
| `core/connection.rs`, `core/database.rs`, `core/vdbe/mod.rs` | `Connection::set_statement_journals` (on by default): off, a write statement in an explicit transaction keeps no statement journal, and one that fails or is abandoned unfinished poisons the transaction, so COMMIT rolls it back whole and says so | Amphora's writer rolls back on any failure; the journal was 13.6% of its time applying rows (Amphora's `docs/design/store.md`, decision 7) |
| `core/database.rs`, `core/storage/pager.rs`, `core/storage/database.rs` | Opening an initialized database defers page checksums on the bootstrap pager (no log attached yet) until the log is attached (`Pager::defer_checksum`, resumed in `Pager::set_wal`, and before a legacy header is rewritten); `IOContext::restore_checksum` | header validation reads page 1 from the file before the log is attached; after a crash during a checkpoint the file's page 1 can be torn while the log holds it whole, and with the `checksum` feature the open failed (Amphora's crash injection: 3-12 of 10,000 power cuts; Turso already clears what the bootstrap read once the log is attached) |
| `core/storage/sqlite3_ondisk.rs` | `checksum_wal` sums a page's worth in four lanes side by side and joins them with powers of the step's matrix (the same sums as one step after another; a test compares every length to 4,168 bytes, both byte orders) | a commit's frames are checksummed one after another on the writer: the chain of dependent adds was 5-6% of Amphora's writer's time |
| `core/storage/page_cache.rs`, `core/storage/pager.rs` | The spill check keeps the exact count of evictable pages its full scan makes (`needs_spill` and `check_spill` take `&mut self`, under the cache's write lock) | a commit cleans its pages in the WAL without telling the cache, so the tracked count only fell; once it was low, every page brought in scanned the whole cache (5% of Amphora's writer's time) |

Taking a newer upstream means copying the four crates again from that commit, re-applying the
changes above, and updating this file.
