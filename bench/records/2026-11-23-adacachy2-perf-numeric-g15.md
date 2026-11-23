# Numeric lane G15 on adacachy2: decimal.rs against the real decimal.c (2026-11-23)

Ada: REALs follow https://www.sqlite.org/floatingpoint.html exactly; where sqlite3 3.54 differs
from the page, the page wins; where the page is silent, sqlite3 3.54.0 decides. Deliverable: G13's
decimal.rs matches ext/misc/decimal.c. Started 21:19Z from main a8962b02; an API error stopped
the lane from about 21:30Z to 21:50Z.

## Verdict, gaps first

- **Not done: decimal_sum's window inverse.** decimal.c registers decimal_sum with
  sqlite3_create_window_function and an xInverse that subtracts the leaving row, so the running
  value keeps the fraction digits of rows that have left the frame. 3.54.0, `decimal_sum(x) OVER
  (ROWS BETWEEN 1 PRECEDING AND CURRENT ROW)` over '1.5','2','3' is `1.5|3.5|5.0`; `ROWS BETWEEN 1
  FOLLOWING AND 1 FOLLOWING` is `2.0|3.0|0.0` (the empty last frame is the accumulator, not
  NULL); `ROWS BETWEEN CURRENT ROW AND CURRENT ROW` over '0.25','-1','1' is `0.25|-1.00|1.00`.
  Amphora's extension aggregate API (`AggFunc`: step and finalize only) has no inverse or value
  call, so decimal.rs cannot express it; the window engine (translate/window.rs AGGINVERSE) has
  to call one. Needs an `AggFunc` inverse/value hook and its window wiring: successor.
- **Not checked this hour:** how Amphora's window engine runs decimal_sum today (recompute or
  refuse); the error code of "out of memory" (sqlite3_result_error_nomem is SQLITE_NOMEM, the
  extension Value carries ResultCode::CustomError); decimal_sum hitting out of memory in a step
  (3.54.0 fails at that step, Amphora at finalize).
- **A question for Ada:** floatingpoint.html 2.2.5 says decimal_sum is "an aggregate, like the
  built-in total() aggregate function". total() over no rows is 0.0; 3.54.0's decimal_sum over no
  rows is NULL. Kept NULL (the page does not state the empty result).

## decimal.c 3.54.0 (sqlite/sqlite tag version-3.54.0) against decimal.rs, function by function

Expected values are sqlite3 3.54.0 (2026-10-09) on adacachy2, `sqlite3 -batch :memory:`.

| decimal.c | difference in decimal.rs (main a8962b02) | now |
|---|---|---|
| decimalNewFromText | no SQLITE_DECIMAL_MAX_DIGIT (10,000,000): `decimal('12e9999999')` built 10,000,001 digits; 3.54.0: out of memory | fixed |
| decimal_new | NULL became a NULL decimal; decimal.c returns no decimal, so `decimal_add(NULL,1)`, `(1,NULL)`, `(NULL,NULL)`, `decimal_sub(NULL,1)` are out of memory and `decimal_sub(1,NULL)`, `decimal_sub(NULL,NULL)`, `decimal_mul` and `decimal_cmp` with a NULL are NULL | fixed |
| decimal_result | same output; isNull is never set by any 3.54.0 path, so the field is gone | same |
| decimal_round | missing: `decimal(X,N)` and `decimal_exp(X,N)` were arity errors | fixed: N significant digits, half away from zero, later digits zero (`decimal('9996',3)` = `10000`, `decimal('0.0996',2)` = `0.1000`, `decimal('-1.25',2)` = `-1.30`); N read as sqlite3_value_int (`'2'`, `2.9`, `4294967298` all 2) |
| decimal_result_sci | no N: trailing zeros always dropped | fixed: keeps N digits (`decimal_exp('1200',3)` = `+1.20e+03`) |
| decimal_cmp | none in value order | kept, page wins: 3.54.0 counts a zero that "1e-2" or "0 012" leaves before the point, so `decimal_cmp('1e-2','0.01')` = 1, `decimal_cmp('1e-2','0.5')` = 1, `decimal_cmp('0 012','12')` = 1; floatingpoint.html 2.2.4: "The result will be negative, zero, or positive if A is less than, equal to, or greater than B". Amphora: 0, -1, 0 |
| decimal_expand | no MAX_DIGIT: `decimal_add('1e9999998','1')` computed; 3.54.0: out of memory (`decimal_add('1e9999997','1')` is 9,999,998 long in both) | fixed |
| decimal_add | same arithmetic, `-0` included (`decimal_add('-1','1')` = `-0`) | same |
| decimalMul | no MAX_DIGIT: `decimal_mul('1e5000000','1e5000000')` would multiply 5e6 by 5e6 digits; 3.54.0: NULL | fixed |
| decimalPow2 | same | same |
| decimalFromDouble | NaN/infinity check was `!is_finite`; decimal.c takes the sign off only when `r < 0.0`, so a NaN BLOB with the sign bit is a number: `decimal(x'FFF8000000000000')` is 1027 characters, `decimal_exp` `+8.344026969402005186588…3359375e-309` (page silent on NaN bits) | fixed |
| decimalFunc | see decimal_round; NULL X with N is NULL | fixed |
| decimalCollFunc | same order as decimal_cmp | kept, page wins: 2.2.6 "compares decimal text strings in numeric order"; 3.54.0 sorts `0.5|1e-2|9|10`, Amphora `1e-2|0.5|9|10` |
| decimalAddFunc, decimalSubFunc | NULLs, see decimal_new | fixed |
| decimalSumStep, Value, Finalize | step and finalize match (no rows NULL, only NULLs `0`, `1, NULL, '2.50', x'31'` = `4.50`) | same |
| decimalSumInverse | missing | not done (gaps) |
| decimalMulFunc | NULLs; product past MAX_DIGIT is NULL | fixed |
| decimalPow2Func | out of range raised G13's own message; 3.54.0: out of memory. 3.54.0 reads N as a C int (`decimal_pow2(4294967297)` = `+2.0e+00`) | fixed to out of memory; the wrap kept out, page wins: 2.2.2 "N must be an integer between -20000 and +20000" |

Empty and malformed text match 3.54.0 unchanged: `decimal('')` = `0`, `decimal_exp('')` =
`+0.0e+00`, `decimal('abc')` = `0`, `decimal('1e')` = `1`, `decimal('1e1x2')` = `1000000000000`,
`decimal('1x2.5')` = `12.5`, `decimal('.5e-1')` = `0.05`, `decimal_add('  -  5','1')` = `-4`;
huge exponents: `length(decimal('1e9999999'))` and `length(decimal('1e99999999999'))` = 10000000.

## Checks

On adacachy2 (`on` 00aad8daff2fa3c9daa65d53c425d450, sync false,
`~/.cache/amphora-perf/numeric-remote`), `CARGO_HOME=~/.cache/cargo-home-perf-numeric
CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=~/.cache/amphora-perf-target/numeric CARGO_INCREMENTAL=0`;
rustfmt `--edition 2021 --check crates/engine/decimal.rs` clean.

| run | head | needs | result |
|---|---|---|---|
| r1609 | fa5037ff | cpus=8,mem=24g | clippy `--all-targets --features bench -D warnings` clean; lib `decimal:: numeric:: ieee754:: printf` 70 passed; `--test floating_point_doc` 48 passed |
| r1623 | be35090e (7bcaeb61 merged with main) | cpus=6,mem=16g (adacachy2 had 7 idle cores; jobs and test threads 6) | clippy clean; lib 72 passed; `floating_point_doc` still running at the merge (48 passed on fa5037ff) |

    cargo clippy -p amphora-engine --all-targets --features bench --locked -- -D warnings
    cargo test -p amphora-engine --features bench --locked --lib -- decimal:: numeric:: ieee754:: printf
    cargo test -p amphora-engine --features bench --locked --test floating_point_doc
