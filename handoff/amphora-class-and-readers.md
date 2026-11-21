# Handoff: amphora-class-and-readers (2026-11-09)

Goals 11209e8d (the replicated class stated once per group) and 314d704a (the local readers bounded by derivation, not
READERS = 4).

## Branch
t1/class-and-readers, based on main db991a0. Not pushed, not merged.
- Head 8d4c05b, tree 2cf6eea63de6c3c74200a6640f67b3517d57c61e.
- Commits:
  - fb18105: design
  - 1a5ec04: red tests
  - 4cfb126: amendments
  - 552c0a1: code
  - faa72f1: refinement text
  - 8d4c05b: review fixes

## What changed
- **Class.** New `crates/amphora/src/replication/class.rs`: the entry `amphora.class.v1\0` + postcard(Durability), the
  `__amphora_class` record, compatibility, the join, and recorded_all.
  - Machine: the record is loaded at open and on install; Invalid at open for an incompatible stated class; apply
    supersedes when compatible and leaves the entry inert when not; Fatal for a member that isn't compatible.
  - Actor: propose_class once a term; class_in_flight holds the leader's writes; keep_class_gate holds suspect_held on
    every other member until stated ∩ configuration ⊆ record.
  - The log changed append-only (new prefix, one table). The transport wire is unchanged (VERSION 6). Durability and
    Fates' postcard encoding is now frozen.
- **Readers.**
  - Engine `SharedPages::charge`/`Charge`, with a row in ORIGIN.md.
  - replicated.rs: a process-wide Account (idle readers oldest first, out ≤ CONNECTION_MAX a replica, closes outside
    the lock) replaces the per-replica pool. `db.rs` exports Charge. `transport::CONNECTION_MAX` is pub(crate).
- **Docs.** docs/design/replication.md has two new sections and their amendments. docs/requirements.toml:
  AC-T1.10.L2 reworded; T-T1.8.C2-C5 and T-T1.10.L8-L9 added; T-T1.10.L5 marked superseded.
- **The batch should know:**
  - Every group's log gains a class entry after its first election, persisted groups included.
  - The test harness's Driven::follow waits for the class to be recorded.
  - At most one follower-prepared write per recording is refused "advanced since preparation" (a stated cost).

## Tested (adas-mac-studio.local-2, at the head's tree, loads 7-18)
- Red first on local-2:

| Test | Red at | Red because |
|---|---|---|
| C2 | 1a5ec04 | "the member stating another class was never refused" |
| C3 | 1a5ec04 | "a held quorum committed 6 before the class applied" |
| L8 | 1a5ec04 | "a held read never had its reader" |
| L9 | 1a5ec04 | "A's idle reader is charged" |
| engine a_charge_shrinks_what_the_pool_keeps | 1a5ec04 | "256 kept beside 128 charged" |
| a_fate_not_yet_recorded_opens_no_held_count | 3efee33 | "after entry 1: left true, right false" |

- Green:
  - `cargo test -p amphora --no-fail-fast`, twice: lib 161 passed, 2 ignored; every integration binary passed.
  - amphora-engine lib: 2,664 passed, at 552c0a1. The engine is unchanged since.
  - `cargo clippy -p amphora -p amphora-api -p amphora-engine --all-targets -- -D warnings`: clean.
  - `cargo fmt --all --check`: clean.
  - `scripts/check-production.sh`: exit 0. On darwin the engine ratchet is counted, not checked.

## Left for the batch (yours)
- The seven gates on the merged tree. Linux check-production is where the engine ratchet is checked; Windows MSVC
  clippy covers the new cfg(test) thread-local and `div_ceil`.
- The power-cut seeds, since the class entry is a new log entry under power cuts:
  `cargo test --release -p amphora --test crash -- --ignored`.
  Also the replicated power-cut tests in the lib: `cargo test -p amphora --lib -- power_cut`.
- No benchmark is claimed. If you measure the readers: the suite at 32 clients reading local on a replica, against
  db991a0.

## Keys
- **Seam owner** (amphora-seam-owner 01a1b152…). Its answers are in
  ~/.cache/claude-scratch/amphora/codex-notices/t1-class-and-readers.reply.md.
  - Agreements: amphora-owner-t1-class-and-readers-fb18105-20261109 (direction, plus the blocking change path),
    amphora-owner-t1-class-and-readers-4cfb126-20261109 (amendments agreed, plus the monotone refinement), and
    amphora-owner-t1-class-and-readers-552c0a1-20261109 (code agreed, plus three changes, done in 8d4c05b).
  - Mine: amphora-class-and-readers-design-fb18105-20261109, -amend-4cfb126-, -code-552c0a1-, -review-8d4c05b-.
  - Its answer at 8d4c05b is still owed.
- **Kiln Metrics** (01a14bf7c0ce7451a5fd7c6f0a9764d0). Not answered yet; the seam owner asked to be copied on both
  answers.
  - Gauge `amphora_replica_local_reader_charge_bytes`, key amphora-class-and-readers/names/v1. It isn't wired: once
    agreed, it's a process-wide gauge in meter.rs, set by the Account on every charge and return, plus its row in
    replication.md, where the doc-truth test needs the name.
  - Events `amphora.replication` `class_refused` and `class_not_proposed`, key amphora-class-and-readers/events/v1.

## Tell Duplicate Systems (01a1b1935e1e7787a5d69076ec3e12e2) once it lands
- **Local readers.** The cap of 4 is gone. A replica allows up to CONNECTION_MAX (32) local readers at once, a policy
  bound. Every replica's readers are charged 8 MiB each to the process's shared page pool (a quarter of memory; 256
  MiB where the OS doesn't say, so 32 readers process-wide on macOS and Windows). A read past either waits until its
  deadline, then gets Refusal::Timeout. There's no new refusal reason.
- **The class.** A group's replicated class is now recorded in its log. A member stating an incompatible class is
  fenced (Refusal::Fenced naming the class), or refused Invalid at open.
  - A joiner's fate: reopen members stating it, and it's recorded when one of them leads.
  - A member's fate, `spanning` or the kind can't change in place: remove and re-add the member, or start a new group.
- **ROLLOUT (needs Ada's word).** A binary from before this change fences on the class entry. Upgrade every member of
  a group before a member running the new binary can lead.
