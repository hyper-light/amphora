# Numeric lane G12 on adacachy2: FP_DIGITS per connection, PostgreSQL notes checked (2026-11-23)

Ada: "follow SQLite's docs on floating point numbers, decimals, etc. *exactly*", and "we now speak
*both* SQLite and Postgres". G11 left SQLITE_DBCONFIG_FP_DIGITS rendered but unwired
(`perf/numeric-g11`, f96543f2). This lane wires it per connection, merges it, and checks G11's
PostgreSQL notes against PostgreSQL 19's pages and source.

## Verdict, gaps first

- **FP_DIGITS is read on the paths below, not yet on every path SQLite reads it.** SQLite reads
  `db->nFpDigit` wherever it turns a REAL into text (`vdbeMemRenderNum`). Amphora still writes the
  default 17 in: group_concat (`update_agg_payload`, 15 callers, morsel workers among them), the text
  functions that read a REAL through `to_string`/`cast_text` (length, upper, substr, replace, instr
  and the rest; about 480 `to_string` sites in vdbe and functions), and translate-time constant
  folding (`folded_default`, ALTER's defaults, the optimizer's sample estimates). Each such site now
  passes `FpDigits::DEFAULT` by name where the signature changed, so the gap is greppable.
- **Units 2 and 3 not started** (the 20 sentences' tests with the doc-over-3.54 printf `!` 26-digit
  generator; the quiet-machine A/B of G11's random doubles). Successor.

## Unit 1: FP_DIGITS per connection

`Connection::db_config_fp_digits(n: i32) -> u8` is `sqlite3_db_config(db, SQLITE_DBCONFIG_FP_DIGITS,
n, &out)`: c3ref/c_dbconfig_defensive.html "The first argument is a small integer, between 3 and
23, or zero. The FP_DIGITS setting is changed to that small integer, or left unaltered if the first
argument is zero or out of range." and the second argument receives "the value of the FP_DIGITS
setting, after possibly being modified". Stored per connection (`AtomicU8`, default 17).

The design for the display without a connection: `Value`'s `Display` (and `ValueRef`'s) carries no
connection and writes the default, 17 ("The default value 17, as of SQLite version 3.52.0"). A
statement's conversions do not go through it any more on the wired paths: they call
`Value::to_text_at(digits)` with `program.connection.fp_digits()`. `exec_printf_values`, the public
sqlite3_mprintf-style entry with no connection, writes 17, as SQLite's `p->db ? p->db->nFpDigit : 17`.
quote() and JSON keep `%!0.17g` (3.54 hard-codes 17 there; G10).

| path (SQLite doc) | where | test (`tests/floating_point_doc.rs`, `fp_digits_per_connection`) |
|---|---|---|
| the setting itself | `connection.rs` `db_config_fp_digits` | `g12_db_config_fp_digits_sets_3_to_23_and_reports_the_setting` |
| CAST to TEXT, to BLOB (lang_expr.html) | `Value::exec_cast`, `op_cast`, `op_function`'s casts | `g12_cast_to_text_and_blob_write_at_the_connections_digits` |
| `\|\|`, concat(), concat_ws() | `exec_concat`, `exec_concat_strings`, `exec_concat_ws` | `g12_concatenation_writes_at_the_connections_digits` |
| TEXT affinity on store (datatype3.html) | `apply_affinity_char` (MakeRecord, Affinity, TypeCheck, ArrayEncode) | `g12_text_affinity_stores_at_the_connections_digits` |
| TEXT affinity in a comparison | `Affinity::convert`/`convert_for_compare`, `comparison_slow` | `g12_text_affinity_in_a_comparison_converts_at_the_connections_digits` |
| LIKE on a REAL | `exec_like_converted` | `g12_like_reads_a_real_at_the_connections_digits` |
| printf `%s %q %Q %w %c %j`, a REAL format string | `functions/printf.rs` `coerce_to_string`, `exec_printf_values_at` | `g12_printf_text_forms_write_at_the_connections_digits` |
| Display stays the default | `types.rs` | `g12_value_display_writes_the_default_whatever_the_connection` |

The six places G11 named: `types.rs` Display x2 (the default, by design above), `vdbe/value.rs`
`||` (wired), `vdbe/affinity.rs` TEXT (wired), `functions/printf.rs` x2 (wired).

No timing: the wired paths do one relaxed atomic load per conversion; the render at 17 is G11's
unchanged code.

## Unit 4: docs/design/postgres.md, numbers as text

Checked against postgresql.org/docs/19/datatype-numeric.html and runtime-config-client.html
(fetched 2026-11-23 18:40Z) and ~/Projects/postgres at `REL_19_0` (5c839a2a08). Rewritten with
verbatim quotes and line cites. Corrected:

- **Shortest output is not Rust's at a midpoint.** "the output value is currently never exactly
  midway between two representable values"; PostgreSQL builds Ryu with `STRICTLY_SHORTEST 0`
  (`ryu_common.h:46`), so `acceptBounds = false` (`d2s.c:363-368`). Rust's `{:e}` accepts the bounds
  for even mantissas. G11's "what fp.rs already asks Rust's `{:e}` for" holds except there.
- **numeric = float8 has no cross-type operator.** `pg_operator.dat` has none; numeric to float8 is
  an implicit cast (`pg_cast.dat:79-80`, `'i'`), float8 to numeric assignment only (`:69-70`), so
  the comparison is float8 = float8.

Added: underflow to zero on input is `22003` (§8.1.3 "will cause an underflow error"; subnormals
accepted, `float.c:512-530`), `extra_float_digits` runs -15 to 3 (`guc_parameters.dat:1085-1092`),
the layout threshold `exp >= -4 && exp < 15` (`d2s.c:806`), `-0` printed `-0`, `+inf`/`+Infinity`
accepted by the code though the page names only `inf` and `-inf`, the int2/int4/int8 cast errors.

## Checks

| run | head | what | result |
|---|---|---|---|
| r1398, r1406 | be9b0446, 10cc95dd | clippy | compile errors (`program` not in scope, a closure's lifetime), fixed by 10cc95dd and 71db20b4 |
| r1411 | 71db20b4 | clippy plain and `--features bench`; `--test floating_point_doc --test real_text`; lib `numeric:: printf json:: vdbe::value vdbe::affinity g9_` | clippy clean both; 21 + 2 passed; lib 282 passed, 0 failed |
| r1420 | 7d11f14a (merged with main d5e901f3; pushed to main) | the same | clippy clean both; 21 + 2 passed; lib 282 passed, 0 failed |

## Successor

1. The rest of FP_DIGITS: group_concat (thread digits through `update_agg_payload` and the morsel
   workers' copy), the text functions' `to_string`/`cast_text` reads of a REAL (a `to_text_at`
   sweep, then `Display` used for display only), translate-time folding with the connection.
2. G11's unit 2 list (20 sentences, a test each, sqlite3 3.54.0 corpus on adacachy2, which has
   `sqlite3` 3.54.0); printf `!` to 26 digits per the doc with an exact generator past 19 digits.
   The `g11_fp_digits_keeps_that_many_significant_digits` test still holds 3.54's 19-digit decode at
   FP_DIGITS 20 (`0.1000000000000000056`); under the doc-first decision that becomes 20 exact digits.
3. G11's random-double A/B on Mac Studio 2 (needs `macos/arm64,cpus=8,mem=24g`, sync from the clone).
4. The Postgres dialect's shortest output with open bounds (unit 4 above).

## Commands

On adacachy2 (`on` 00aad8daff2fa3c9daa65d53c425d450, needs `linux/amd64,cpus=4,mem=16g`, sync false,
`~/.cache/amphora-perf/numeric-remote` at the commit named), `CARGO_HOME=~/.cache/cargo-home-perf-numeric
CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=~/.cache/amphora-perf-target/numeric CARGO_BUILD_JOBS=4
CARGO_INCREMENTAL=0`:

    cargo clippy -p amphora-engine --all-targets [--features bench] --locked -- -D warnings
    cargo test -p amphora-engine --features bench --locked --test floating_point_doc --test real_text -- --test-threads=4
    cargo test -p amphora-engine --features bench --locked --lib -- numeric:: printf json:: vdbe::value vdbe::affinity g9_ --test-threads=4
