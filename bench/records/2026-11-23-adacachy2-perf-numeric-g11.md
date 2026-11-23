# Numeric lane G11 on adacachy2: REAL to text without the re-parse, FP_DIGITS, the linked pages (2026-11-23)

Ada: "we should follow SQLite's docs on floating point numbers, decimals, etc. *exactly*", and "we
now speak *both* SQLite and Postgres". G10 made the SQLite dialect write a REAL as
floatingpoint.html 1.3.1 says (15 digits when they convert back, else 17) at the cost of one extra
format and parse per REAL. This lane measures and removes that cost without changing an output,
adds SQLITE_DBCONFIG_FP_DIGITS, reads the pages floatingpoint.html links, and lists the Postgres
dialect's number rules.

## Verdict, losses first

- **random doubles: no measurable difference, possibly 10% slower.** Median of 5 alternated rounds
  204.1 ns per value before, 224.2 after; ranges 200-229 and 201-271 overlap, on a host at load
  25-30 of 32 cores. 94% of random doubles need 17 digits, where both builds do two conversions
  (before: 15 exact digits and a parse; after: the shortest digits).
- **q01's charge (`l_extendedprice*(1-l_discount)*(1+l_tax)`): 181.8 -> 135.7 ns per value (-25%).**
  2-place prices: 171.7 -> 93.0 ns (-46%). Every text identical (below).
- **q01 end to end was not run.** q01 writes 4 rows of 8 REALs at any scale, 32 texts, so the saving
  is about 1.5 us per execution against a query that runs for hundreds of milliseconds at SF1:
  inferred, under 0.01%. The micro cost matters where every row writes a REAL as text (CAST in a
  projection, printf, `||`, results rendered as text); no end-to-end query of that shape ran.
- **FP_DIGITS is not on main.** The rendering at any setting and its tests are on `perf/numeric-g11`
  (f96543f2); the per-connection setting and its readers are not wired (below), so it stays off main.
- **The linked pages are read and quoted, not yet tested rule by rule.** Two places where SQLite
  3.54.0's code departs from its docs are listed; neither is changed on main.

## Unit 1: the 15-digit test from the shortest digits (main)

`crates/engine/numeric/fp.rs` `fifteen_digits`: for a normal double, Rust's shortest round-trip
`{:e}` (exact: Grisu with Dragon4 fallback, bounds inclusive for even mantissas, as round-half-even
parsing reads them) written into a 32-byte stack buffer. More than 15 digits: step (c), 17 digits.
Otherwise those digits are step (a)'s, and step (b) holds:

- half an ulp of a normal double is at most 2^-53 (1.11e-16) of it, and half a unit in the 15th
  digit is at least 5e-16 of it, so the shortest text (within half an ulp) is the unique nearest
  15-digit decimal, which is what rounding to 15 digits gives;
- if 15 digits converted back, a text of at most 15 digits round-trips, so the shortest has at most
  15; if the shortest has more, no 15-digit text converts back.

A subnormal's ulp is wider than that bound (`5e-324` round-trips; the page's 15 digits are
`4.94065645841247e-324`), so subnormals keep G10's written steps (`{:.14e}`, parse).

Identity, both builds over the same inputs (r1299, `examples/real_text.rs`):

| input | doubles | texts that differ from main's |
|---|---|---|
| G10's corpus (`~/.cache/amphora-g10/corpus.tsv`: fixture, random bits, 0-15 place decimals, products), each double's stored text | 304,713 | 0 before, 0 after |
| q01 charge, 2-place prices, random finite bits: FNV-1a of every text | 3 x 1,000,000 | hashes equal in all 10 runs (a46a2580..., 89eb310c..., e77fabb5...) |

The A/B (r1299; A = 3fedc10f main, B = 89307344, whose fp.rs differs from the landed head only by
the FP_DIGITS rendering, `cargo build --release --features bench --example real_text`, 5 rounds
alternated A,B then B,A, each 7 timed passes of 1,000,000 values after an untimed pass; ns per
value, each round's median):

| class | A rounds | B rounds | A median | B median |
|---|---|---|---|---|
| q01-charge (56.6% need at most 15 digits) | 196.4 180.0 177.4 322.9 181.8 | 135.2 150.1 133.5 138.0 135.7 | 181.8 | 135.7 |
| price-2dp (100%) | 185.3 166.7 162.4 187.9 171.7 | 93.9 95.8 91.1 93.0 85.4 | 171.7 | 93.0 |
| random-bits (6.1%) | 228.6 200.3 205.4 201.3 204.1 | 229.9 224.2 217.2 271.2 200.9 | 204.1 | 224.2 |

Load 25.3-30.8 (1 min) on 32 cores, CPU pressure avg10 2.2-19.5%, I/O 4.4-8.7% through the run;
needs `linux/amd64,cpus=4,mem=16g`. A's round 4 (322.9) ran beside a load spike. No calibration is
involved: the example calls `numeric::format_float` and opens no database. The limiter after the
change: for 15-digit values, the shortest-digit search itself and the `String` the render returns;
for 17-digit values, the shortest search plus `decode`, which a shared digit generation could
merge (successor).

## Unit 2: SQLITE_DBCONFIG_FP_DIGITS (perf/numeric-g11, not on main)

c3ref/c_dbconfig_defensive.html, verbatim:

> The SQLITE_DBCONFIG_FP_DIGITS setting is a small integer that determines the number of
> significant digits that SQLite will attempt to preserve when converting floating point numbers
> (IEEE 754 "doubles") into text. The default value 17, as of SQLite version 3.52.0. The value was
> 15 in all prior versions.

> This option takes two arguments which are an integer and a pointer to an integer. The first
> argument is a small integer, between 3 and 23, or zero. The FP_DIGITS setting is changed to that
> small integer, or left unaltered if the first argument is zero or out of range. The second
> argument is a pointer to an integer. If the pointer is not NULL, then the value of the FP_DIGITS
> setting, after possibly being modified by the first arguments, is written into the integer to
> which the second argument points.

floatingpoint.html: "If you do not want the (negligible) overhead of step (b) and (c), just set
SQLITE_DBCONFIG_FP_DIGITS to 15."

On the branch: `fp::FpDigits` (3 to 23, default 17), `fp::real_to_text_digits(r, digits)`: at 17 the
page's steps, else `%!.*g` at that many digits (sqlite3.c 3.54.0 `vdbeMemRenderNum`: `"%!.*g",
(p->db ? p->db->nFpDigit : 17)`), exponent form past `digits - 1`. Four tests in
`tests/floating_point_doc.rs` (`g11_fp_digits_*`, under the `bench` feature) quote the sentences;
r1299 ran them: 12 passed, 1 failed (`0.1` at 20 digits: 3.54's decode carries 19 digits,
`0.1000000000000000056`; f96543f2 holds that text). Not wired: the connection setting and its
readers, `types.rs:854` and `:2439` (`Value`'s Display), `vdbe/value.rs:1688` (`||`),
`vdbe/affinity.rs:230` (TEXT affinity), `functions/printf.rs:193` and `:1413`. `Value`'s Display
carries no connection, so that is the design question for the successor.

**SQLite differs from its doc:** the doc takes "between 3 and 23"; 3.54.0's `if( nIn>3 && nIn<24 )`
refuses 3. Amphora follows the doc.

## Unit 3: the pages floatingpoint.html links, verbatim

Fetched 2026-11-23 17:35Z. printf.html "last updated on 2026-10-28 18:42:03Z"; lang_expr.html
"2026-08-14 19:21:30Z"; datatype3.html and c_dbconfig_defensive.html carry no date.
floatingpoint.html links lang_expr.html#castexpr, c_dbconfig_defensive.html#sqlitedbconfigfpdigits,
lang_aggfunc.html#sumunc, rtree.html, column_blob.html, value_blob.html, cli.html,
amalgamation.html and decimal.c. Not one rule below has its own test yet; the successor writes one
per sentence and a sqlite3 corpus where it applies.

printf.html:

1. "The floating-point to text conversion subfunction for the built-in printf() is limited in
   precision to 16 significant digits or 26 significant digits if the "!" alternate-form-2 flag is
   used."
2. "For floating point substitutions, the alternate-form-2 flag increases the maximum number of
   significant digits displayed from 16 to 26, forces the display of the decimal point and causes
   at least one digit to appear after the decimal point."
3. "For floating-point substitutions %e, %E, and %f the precision specifies the number of digits to
   display to the right of the decimal point. With the %g and %G, the precision is the total number
   of significant digits, rounded up to 1 if the specified precision is 0."
4. "Infinity and NaN (Not-A-Number) floating point values are normally rendered as "Inf" and "NaN",
   respectively, but with the zero-padding option enabled they are rendered as "9.0e+999" and
   "null"."
5. "For %#g and %#G substitutions, this causes trailing zeros to be removed." / "For the %#f
   substitution, minus signs are suppressed if the '+' flag is omitted and if all displayed digits
   are zero." / "This flag forces a decimal point to appear for all floating-point substitutions."
6. "The comma option causes comma separators to be added to the output of numeric substitutions
   (%d, %f, and similar) before every 3rd digits to the left of the decimal point."

**SQLite differs from its doc:** rules 1 and 2 say 26 digits with "!"; 3.54.0 calls
`sqlite3FpDecode(&s, realvalue, iRound, flag_altform2 ? 20 : 16)` and its decode yields at most 19.
Following the doc means a digit generator past 19 digits (exact, e.g. Dragon4) for `%!.Ng`, N > 19.

lang_expr.html (CAST):

7. "Casting an INTEGER or REAL value into TEXT renders the value as if via sqlite3_snprintf() except
   that the resulting TEXT uses the encoding of the database connection."
8. "When casting a TEXT value to REAL, the longest possible prefix of the value that can be
   interpreted as a real number is extracted from the TEXT value and the remainder ignored. Any
   leading spaces in the TEXT value are ignored when converting from TEXT to REAL. If there is no
   prefix that can be interpreted as a real number, the result of the conversion is 0.0."
9. "If the prefix integer is greater than +9223372036854775807 then the result of the cast is
   exactly +9223372036854775807. Similarly, if the prefix integer is less than -9223372036854775808
   then the result of the cast is exactly -9223372036854775808."
10. "For example, "CAST('123e+5' AS INTEGER)" results in 123, not in 12300000."
11. "A cast of a REAL value into an INTEGER results in the integer between the REAL value and zero
    that is closest to the REAL value. If a REAL is greater than the greatest possible signed integer
    (+9223372036854775807) then the result is the greatest possible signed integer and if the REAL is
    less than the least possible signed integer (-9223372036854775808) then the result is the least
    possible signed integer."
12. "Input text that looks like floating point (there is a decimal point and/or an exponent) and the
    text describes a value that can be losslessly converted back and forth between IEEE 754 64-bit
    float and a 51-bit signed integer, then the result is INTEGER." / "Any text input that describes
    a value outside the range of a 64-bit signed integer yields a REAL result." / "Casting a REAL or
    INTEGER value to NUMERIC is a no-op, even if a real value could be losslessly converted to an
    integer."
13. "If a numeric literal has a decimal point or an exponentiation clause or if it is less than
    -9223372036854775808 or greater than 9223372036854775807, then it is a floating point literal."

datatype3.html (affinity and comparison):

14. "For conversions between TEXT and REAL storage classes, about 15.95 significant decimal digits of
    the number are preserved." / "If a floating point value that can be represented exactly as an
    integer is inserted into a column with NUMERIC affinity, the value is converted into an integer."
15. "Hence, the string '3.0e+5' is stored in a column with NUMERIC affinity as the integer 300000,
    not as the floating point value 300000.0."
16. "A column with REAL affinity behaves like a column with NUMERIC affinity except that it forces
    integer values into floating point representation."
17. "An INTEGER or REAL value is less than any TEXT or BLOB value. When an INTEGER or REAL is
    compared to another INTEGER or REAL, a numerical comparison is performed."
18. "If one operand has INTEGER, REAL or NUMERIC affinity and the other operand has TEXT or BLOB or
    no affinity then NUMERIC affinity is applied to the other operand."
19. "When grouping values with the GROUP BY clause values with different storage classes are
    considered distinct, except for INTEGER and REAL values which are considered equal if they are
    numerically equal."
20. "The implied type conversion of mathematical operands is slightly different from CAST to NUMERIC
    in that string and BLOB values that look like real numbers but have no fractional part are kept
    as REAL instead of being converted into INTEGER as they would be for CAST to NUMERIC."

## Unit 4: the Postgres dialect (main, docs only)

`docs/design/postgres.md` §5 "Numbers as text: due when the frontend returns values": float8/float4
output by `extra_float_digits` (shortest exact by default), float4 as its own type, PostgreSQL's
special-value spellings, `22003`/`22P02` input errors, numeric as exact decimal, and the casts and
cross-type comparisons, each through `Dialect` with a test quoting PostgreSQL 19's sentence.

## Checks

| run | head | what | result |
|---|---|---|---|
| r1287 | 99dd87c8 | `--test floating_point_doc --test real_text`; `--lib numeric:: printf json::` | 9 + 2, 231 passed; clippy failed on the example (`numeric` private without `bench`), fixed by 2aa338ce |
| r1299 | 89307344 | clippy plain; clippy `--features bench`; tests with `bench`; lib | plain clippy: `FpDigits::new` unused (why unit 2 stays off main); bench clippy clean; 12 + 2 passed, 1 failed (fixed on f96543f2); lib 231 passed |
| r1299 | 3fedc10f, 89307344 | corpus, A/B | above |
| r1311 | 6911e962 (merged with main a7160678) | clippy plain and `bench`; `floating_point_doc real_text oracle_findings`; lib `numeric:: printf json:: vdbe::value vdbe::affinity g9_` | clippy clean both; 9 + 83 + 2 passed; lib 282 passed, 0 failed |
| after the push | 80c1f31c (main; 11 engine files from main merged in after r1311, none of them numeric) | clippy `--features bench`; the same tests and lib filters | clippy clean; 9 + 83 + 2 passed; lib 282 passed, 0 failed |

## Successor

1. Wire FP_DIGITS per connection (`Connection` field, a `db_config` entry or pragma as Amphora
   exposes them) to every reader listed in unit 2, then land `perf/numeric-g11`'s unit 2.
2. A test per sentence 1-20 with a sqlite3 3.54.0 corpus where it applies; printf's 26 digits with
   "!" (doc) where 3.54 stops at 19-20.
3. Merge the 17-digit path's two digit generations (shortest search, then `decode`) if the random
   doubles' cost is confirmed on a quiet machine.

## Commands

On adacachy2 (`on` 00aad8daff2fa3c9daa65d53c425d450, needs `linux/amd64,cpus=4,mem=16g`, sync false,
`~/.cache/amphora-perf/numeric-remote`), `CARGO_HOME=~/.cache/cargo-home-perf-numeric
CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=~/.cache/amphora-perf-target/numeric CARGO_BUILD_JOBS=4
CARGO_INCREMENTAL=0`, worktrees under `~/.cache/numeric-ab/<sha>`, binaries in
`~/.cache/amphora-perf-bin/numeric/real_text-<sha>` (arm A built with B's example file copied in):

    cargo build --release --locked -p amphora-engine --features bench --example real_text
    real_text-<sha> 1 1 ~/.cache/amphora-g10/corpus.tsv
    for i in 1..5: (A B | B A) real_text-<sha> 1000000 7
