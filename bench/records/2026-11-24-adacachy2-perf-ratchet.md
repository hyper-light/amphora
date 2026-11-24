# Lane F4 (ratchet), 2026-11-24: check-production under the pinned Rust 1.99.1

Ada's goal: a main whose gates mean the same on every machine, so "check-production passed" is true.
The question: lanes reported check-production passing on adacachy2 ("10903 left ... as the ratchet
holds"), while the T1 verifier reported counts above five ceilings under 1.99.1
(arithmetic_side_effects 3492, cast_possible_truncation 568, cast_sign_loss 266, cast_possible_wrap
696, disallowed_methods 69).

## Losses first

- No code changed. Under 1.99.1 every lint counts exactly its ceiling, so nothing was over and
  nothing needed a fix.
- The T1 verifier's five counts did not come back under any of the eight ways of counting below.
  Where they came from is still unknown.

## Which toolchain lanes' runs use

Each run went through kiln's `run`, `on` adacachy2 (00aad8daff2fa3c9daa65d53c425d450), with RULES
rule 6's environment (CARGO_HOME=~/.cache/cargo-home-perf-ratchet with registry, git and bin linked
from ~/.cargo, CARGO_NET_OFFLINE=true, CARGO_TARGET_DIR=~/.cache/amphora-perf-target/ratchet,
CARGO_BUILD_JOBS, CARGO_INCREMENTAL=0), 2026-11-24 04:08Z to 04:20Z.

| Where the run's tree was | RUSTUP_TOOLCHAIN | `rustc -V` | `cargo clippy -V` |
|---|---|---|---|
| A GitHub clone, `sync: false` (rule 6's workaround) | unset | rustc 1.99.1 (58cf8f9c3 2026-10-13) | clippy 0.1.99 (58cf8f9c35 2026-10-13) |
| The same clone with `unset RUSTUP_TOOLCHAIN` | unset | rustc 1.99.1 (58cf8f9c3 2026-10-13) | clippy 0.1.99 (58cf8f9c35 2026-10-13) |
| kiln's reach folder, `sync: true` from the lane's clone | unset | rustc 1.99.1 (58cf8f9c3 2026-10-13) | clippy 0.1.99 (58cf8f9c35 2026-10-13) |

- In all three, `rustup show active-toolchain` named 1.99.1, "overridden by" the tree's
  rust-toolchain.toml.
- The runner passes no RUSTUP_* variable to its runs.
- rustup's one directory override on adacachy2 is a kiln folder, also pinned to 1.99.1.
- The default toolchain there is 1.98.1, and `stable` is rustc 1.100.0 (0ef1bc386 2026-11-09). Neither
  applies inside an Amphora tree.
- The nine lane scripts under ~/.cache/amphora-perf that name RUSTUP_TOOLCHAIN set it to 1.99.1 (for
  hyper-raft clones), set it to the Amphora tree's own active toolchain, or unset it. None names
  stable.
- RULES rule 6 needs no new line, because lanes' runs already use the pin.

Lane PG13's warning that `AtomicU32::fetch_update` was renamed to `try_update` comes from the pin itself.
A probe program calling both was compiled on adacachy2 with each toolchain:

| Toolchain | `fetch_update` | `try_update` |
|---|---|---|
| 1.98.1 | compiles, no warning | compiles |
| 1.99.1 (the pin) | "use of deprecated method ... `fetch_update`: renamed to `try_update` for consistency" | compiles |
| 1.100.0 (stable) | the same warning | compiles |

Main's `try_update` calls (crates/engine/vdbe/morsel.rs, crates/engine/dialect/postgres.rs,
crates/engine/storage/versioned/memory.rs, crates/amphora/src/descriptors.rs,
crates/amphora/src/transport/common.rs, crates/api/src/sql.rs) compile on 1.99.1.

## check-production on main under 1.99.1

`bash scripts/check-production.sh` at main 39ecf1df, in an empty target dir (230 crates compiled or
checked, `amphora-engine` among them; run r222): exit 0,
"engine-ratchet: 10903 left in the engine's production code across 17 lints, as the ratchet holds".
The engine pass's JSON, counted lint by lint the way scripts/engine-ratchet.py counts it:

| Lint | Count | Ceiling |
|---|---|---|
| panic | 198 | 198 |
| unwrap_used | 448 | 448 |
| expect_used | 859 | 859 |
| unreachable | 458 | 458 |
| todo | 11 | 11 |
| unimplemented | 0 | 0 |
| indexing_slicing | 3182 | 3182 |
| arithmetic_side_effects | 3345 | 3345 |
| string_slice | 101 | 101 |
| cast_possible_truncation | 547 | 547 |
| cast_sign_loss | 249 | 249 |
| cast_possible_wrap | 685 | 685 |
| disallowed_macros | 780 | 780 |
| disallowed_methods | 40 | 40 |
| disallowed_types | 0 | 0 |
| dbg_macro | 0 | 0 |
| exit | 0 | 0 |

At 2571f841, the T1 landing the verifier named, the engine pass counts the same 10903. From
2571f841 to 39ecf1df only crates/amphora/src/database/replicated.rs changed under crates/.

Each ceiling equals the pin's count today. Had a ceiling been lowered to a different toolchain's
lower count, 1.99.1 would count above it now; had it been set from a higher one, 1.99.1 would count
below it and check-production would fail asking to lower it. Neither happens, so no ceiling holds
another toolchain's count.

## Other ways of counting, to find the verifier's numbers (not the gate)

All under 1.99.1 on adacachy2, at 2571f841 unless named, with check-production's lints and
CLIPPY_CONF_DIR, counting engine spans (crates/engine*) once per place unless named (runs r241, r253,
r269 and a recount of their saved JSON).

| Way of counting | arithmetic_side_effects | cast_possible_truncation | cast_sign_loss | cast_possible_wrap | disallowed_methods |
|---|---|---|---|---|---|
| The T1 verifier's report | 3492 | 568 | 266 | 696 | 69 |
| check-production's engine pass (the gate) | 3345 | 547 | 249 | 685 | 40 |
| The same, one count per line | 3283 | 540 | 244 | 676 | 40 |
| The same, every message (no de-duplication) | 3352 | 550 | 249 | 685 | 40 |
| Engine crates, `--all-targets` (at 39ecf1df) | 4140 | 815 | 344 | 826 | 78 |
| `--workspace` (features unified across the workspace) | 3349 | 547 | 249 | 687 | 42 |
| `--workspace --all-targets` | 4155 | 825 | 344 | 830 | 80 |
| Engine pass, `--target aarch64-apple-darwin` | 3345 | 544 | 249 | 685 | 40 |
| Engine pass, `--target x86_64-pc-windows-msvc` | 3349 | 547 | 247 | 680 | 40 |

None gives the verifier's five numbers. Not tried: a count taken on macOS itself, and a commit
other than 2571f841 and 39ecf1df.

## Noted, not acted on

- `cargo clippy --workspace` builds the engine with the features other workspace crates turn on.
  That build has 12 production sites in crates/engine* that check-production's engine pass
  (`-p` the four engine crates, their own features) never sees: arithmetic_side_effects +4,
  cast_possible_wrap +2, disallowed_macros +3, disallowed_methods +2, indexing_slicing +1.

## Commands

The count (r222; r241, r253 and r269 ran the other passes the same way, at CARGO_BUILD_JOBS=3 under
needs "linux/amd64,cpus=3,mem=10g", after adacachy2 refused cpus=8 and cpus=4 for want of idle cores):

```
cd ~/.cache/amphora-perf/ratchet-remote && git fetch -q origin && git checkout -q --detach origin/main
export CARGO_HOME=$HOME/.cache/cargo-home-perf-ratchet CARGO_NET_OFFLINE=true \
  CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/ratchet CARGO_BUILD_JOBS=3 CARGO_INCREMENTAL=0
unset RUSTUP_TOOLCHAIN
nice -n 10 bash scripts/check-production.sh
CLIPPY_CONF_DIR="$PWD" nice -n 10 cargo clippy -p amphora-engine -p amphora-engine-parser \
  -p amphora-engine-macros -p amphora-engine-ext --locked --no-deps --message-format=json \
  -- -W clippy::<each of check-production's 17 lints> > engine.json
```

The other passes added `--all-targets`, `--workspace` (in place of the four `-p`), or
`--target <triple>` to that last command. The probe:

```
for tc in 1.99.1 1.98.1 stable; do rustup run $tc rustc --edition 2024 -o p-$tc p.rs; done
```
