# Numeric lane G13 on adacachy2: every sentence of floatingpoint.html as a test (2026-11-23)

Ada: REALs follow https://www.sqlite.org/floatingpoint.html exactly; where the page and sqlite3 3.54
differ, the page wins. Rule 0 deliverable (coordinator, 19:16Z): every sentence of the page has a
test, and it passes, merged. Started 19:12Z from main 4a62875e.

## Verdict, gaps first

- **Not done in this hour:** printf's `!` to 26 digits, the paths still at 17 (group_concat, the
  text functions, folding at prepare), the Postgres items, and G11's FP_DIGITS-20 test (still
  3.54's 19 digits). Successor.
- **Sentences with no test, each with its reason:** "Hence there are 1.845e+19 different possible
  floating point values" (a count of 2^64, no engine behaviour); ".dbconfig fp_digits 15" (Amphora
  has no sqlite3-style shell; the C API form is g12_db_config_fp_digits_*); advice and history with no
  behaviour of their own ("you should not use binary-64 values", "This is not an SQLite limitation",
  "no such guarantees are possible", "more pleasing to human readers", "There are also performance
  reasons for rounding", the NB paragraph); NaN in "A binary-64 can also be ... NaN" is stored as
  NULL, as sqlite3_result_double does (ieee754_from_blob of a NaN, measured in 3.54.0), untested.
- **Section 2 needed code.** The page's ieee754 (§2.1) and decimal (§2.2) extensions did not exist
  in Amphora. SQLite's CLI includes both by default and Amphora already builds in the CLI's regexp
  and series, so both are now built in: crates/engine/ieee754.rs, crates/engine/decimal.rs (the
  decimal port written by a delegate from decimal.c's behaviour, checked against 3.54.0 below), the
  "decimal" collation in the database's built-in symbol table (crates/engine/ext/mod.rs).

## The page, sentence by sentence

| sentence (floatingpoint.html) | test (crates/engine/tests/floating_point_doc.rs) |
|---|---|
| 64-bit twos-complement integers, -9223372036854775808 to +9223372036854775807, exact | g10_integers_are_exact_over_the_64_bit_range |
| REALs are IEEE 754 binary-64; "uses 64 bits per number" | g13_a_real_is_64_bits |
| range 1.7976931348623157e+308 to 4.9406564584124654e-324, either sign | g13_reals_range_from_the_largest_to_the_smallest_binary64_either_sign |
| 0.0 and -0.0, the infinities | g10_reals_hold_signed_zero_and_infinities |
| "Floating point values are approximate." (three times) | g13_floating_point_values_are_approximate |
| "preserve as many significant digits ... as it can" | g10_displayed_text_converts_back_to_the_same_double |
| 1152693165.1106291898 - 1152693165.1106280772 is 0.00000095367431640625 in binary-64 | g10_subtraction_of_near_equal_reals_gives_the_documented_binary64, g13_the_near_equal_subtraction_is_exactly_the_pages_binary64 |
| M × 2^E; 47.49 is 6683623321994527 × 2^-47 | g13_a_real_is_m_times_2_to_the_e |
| M 53 bits, E from -1074 to +972 | g13_e_runs_from_minus_1074_to_972 (972 is the infinities' E, as ieee754() writes it) |
| 47.49's double and its lower neighbour, "the first number is closer" | g10_decimal_text_takes_the_closer_double, g13_47_49_and_the_next_smaller_double |
| only 0.00, 0.25, 0.50, 0.75 cents are exact | g13_only_quarter_cents_are_exact |
| 0.0000000301 dollars per second, 66 nanoseconds, the stored value's 48 digits | g13_the_error_in_47_49_is_66_nanoseconds_of_inflation |
| text for display is rounded (column text, CAST) | g10_47_49_displays_as_47_49 |
| "all 49 signficant digits" | g13_47_49_has_49_significant_digits |
| 17 or more digits round-trip | g10_displayed_text_converts_back_to_the_same_double, g11_fp_digits_keeps_that_many_significant_digits |
| 15 or fewer often round-trip, not always | g13_15_digits_round_trip_often_but_not_always |
| 1.23 and 2.34 as M × 2^-51 and their exact values | g13_1_23_and_2_34_as_binary64 |
| the exact product of the two doubles | g13_the_exact_product_of_1_23_and_2_34 |
| the hardware's product, 6481130223748880 × 2^-51 | g13_the_binary64_product_of_1_23_and_2_34 |
| 15 digits give 2.8782, 17 give 2.8781999999999997 | g10_product_that_needs_17_digits_displays_them, g13_15_digits_round_trip_often_but_not_always |
| 2.8782 and 2.8781999999999997 differ by one in M | g13_2_8782_and_2_8781999999999997_differ_by_one_in_m |
| (B) is the default since 3.52.0 | g11_fp_digits_default_is_17_and_takes_the_pages_steps |
| sqlite3_db_config(db, SQLITE_DBCONFIG_FP_DIGITS, 15, 0) | g12_db_config_fp_digits_sets_3_to_23_and_reports_the_setting |
| the three steps (15 digits, convert back, else 17) | g10_display_is_15_digits_when_they_convert_back_else_17, g10_real_text_corpus |
| "just set SQLITE_DBCONFIG_FP_DIGITS to 15" | g11_fp_digits_15_rounds_to_15_always |
| §2.1.1 ieee754(F) and ieee754(M,E) | g13_a_real_is_m_times_2_to_the_e, g13_ieee754_of_m_and_e_is_the_real |
| §2.1.2 ieee754_mantissa, ieee754_exponent | g13_a_real_is_m_times_2_to_the_e |
| §2.1.3 ieee754_to_blob, ieee754_from_blob | g13_ieee754_blobs_are_the_big_endian_encoding, g13_reals_range_* |
| §2.2.1 decimal_add, decimal_sub, decimal_mul return text, numeric inputs as text | g13_decimal_add_sub_mul_return_text |
| §2.2.1 no decimal_div | g13_there_is_no_decimal_div |
| §2.2.2 decimal_pow2 exact, N from -20000 to +20000 | g13_decimal_pow2_is_exact_between_minus_and_plus_20000 |
| §2.2.3 decimal and decimal_exp, a REAL expanded exactly | g13_decimal_and_decimal_exp_expand_exactly |
| §2.2.4 decimal_cmp's sign | g13_decimal_cmp_orders_by_value |
| §2.2.5 decimal_sum is precise | g13_decimal_sum_is_precise |
| §2.2.6 the decimal collation | g13_the_decimal_collation_orders_numerically |

## Where sqlite3 3.54.0 departs from the page (adacachy2, `sqlite3 :memory:`, 3.54.0 2026-10-09)

- `ieee754_from_blob(x'0000000000000001')`: the page shows `4.94065645841247e-324`, 3.54.0 writes
  `4.9406564584124654e-324`. The page's 15 digits convert back, so 1.3.1's steps write them; Amphora
  follows the page (G10's subnormal path).
- `decimal_pow2(20001)`: 3.54.0 raises "out of memory". The page says N "must be" within ±20000;
  Amphora raises "decimal_pow2(N): N must be an integer between -20000 and +20000".
- Found on the way, not this deliverable: `printf('%.17g', 47.49)` is `47.49` in 3.54.0, where
  printf.html's "the precision is the total number of significant digits" gives 47.490000000000002;
  `decimal_add(NULL, 1)` raises "out of memory" in 3.54.0, Amphora returns NULL (the page is silent).

3.54.0's answers used as expected values where the page shows none: decimal_exp's leading sign
(`+4.749...e+01`), decimal_pow2's scientific form (`+5.0e-01`, `+1.024e+03`; lengths 6029 and 13988
at ±20000), decimal_mul trimming to the shorter input fraction (`decimal_mul('0.5','0.20')` = `0.1`),
add/sub/mul reading a REAL as its text (`decimal_mul(1.23, 2.34)` = `2.8782`), quote of the blob of
47.49 (`X'4047BEB851EB851F'`), the cents `0,25,50,75`, the inflation figures `3.01e-08` and `66.0`.

## Checks

On adacachy2 (`on` 00aad8daff2fa3c9daa65d53c425d450, needs `linux/amd64,cpus=8,mem=24g`, sync false,
`~/.cache/amphora-perf/numeric-remote` at the commit named; the macOS placement r1448 was lost, the
Mac could not reach this machine for the sync), `CARGO_HOME=~/.cache/cargo-home-perf-numeric
CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=~/.cache/amphora-perf-target/numeric CARGO_BUILD_JOBS=8
CARGO_INCREMENTAL=0`:

| run | head | what | result |
|---|---|---|---|
| r1476 | 6a10d6f2 | clippy `--all-targets --features bench`; lib `decimal:: ieee754::` | clippy: `Decimal::is_null` unused (removed in 73dfa992); 23 passed |
| r1486 | 73dfa992 (merged with main 392d9aac) | the same clippy; `--test floating_point_doc`; lib `decimal:: ieee754:: numeric:: printf json::` | page tests did not compile (`IO` not imported, fixed in feacf80c); lib 241 passed, 0 failed |
| foreground run, 12:02 adacachy2 clock (no id returned) | feacf80c (pushed to main) | clippy; `--test floating_point_doc` | clippy clean; 45 passed, 0 failed |

    cargo clippy -p amphora-engine --all-targets --features bench --locked -- -D warnings
    cargo test -p amphora-engine --features bench --locked --test floating_point_doc -- --test-threads=8
    cargo test -p amphora-engine --features bench --locked --lib -- decimal:: ieee754:: numeric:: printf json:: --test-threads=8
