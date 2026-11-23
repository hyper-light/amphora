# Numeric lane G10 on adacachy2: the SQLite dialect's numbers follow floatingpoint.html (2026-11-23)

Ada's decision: "https://www.sqlite.org/floatingpoint.html - we should follow SQLite's docs on
floating point numbers, decimals, etc. *exactly*." Amphora is SQLite. Then: "we now speak *both*
SQLite and Postgres", so number rules are per dialect, never one global behaviour. This lane: the
SQLite dialect follows the page exactly, input and output, with one test per rule and a corpus
count against sqlite3 3.54.0.

## Verdict, losses first

- **REAL to text now differs from SQLite 3.54.0 where 3.54 departs from the page.** The page's
  algorithm writes 15 significant digits whenever they convert back to the same double. 3.54's code
  tries fewer than 17 digits only when the 14th to 16th digits are zeros or the 15th and 16th are
  nines, so it writes `716936.91809735901` where the page's steps give `716936.918097359`, and
  `4.9406564584124654e-324` where they give `4.94065645841247e-324` (the page's own ieee754 example
  shows that text). Amphora now writes the page's text. On 304,713 doubles (r1251), Amphora writes the page's text for all 173,493 where the page writes 15 digits; SQLite 3.54.0 writes it for 168,532 (97.141%) and 17 digits for the other 4,961.
- **The page's 17-digit rounding is not correct rounding.** The page says "If you round to 17
  digits, you get 2.8781999999999997" for 6481130223748880 x 2^-51, whose exact value is
  2.87819999999999964757...; correctly rounded to 17 digits that is 2.8781999999999996. The page's
  answer comes from rounding to 18 digits first, then rounding at the 18th. Amphora reproduces the
  page's answer (SQLite's sqlite3FpDecode at 17, kept from the 3.54 port). Ada's choice is below.
- **Not done this hour:** `SQLITE_DBCONFIG_FP_DIGITS` (the page's option to round at 15 digits
  always), the pages the doc links (printf.html, lang_expr CAST, the decimal and ieee754
  extensions' own docs), and a fresh read of SQLite's C source (the names below come from the 3.54
  port's own citations in `numeric/fp.rs`). The successor takes them, listed at the end.
- **The Postgres dialect has no number path on main to keep apart yet.** `Dialect` has one
  implementation (`SqliteDialect`); `crates/amphora/src/postgres` (main 5e6c619f) does startup and
  SCRAM only and renders no values. `numeric::format_float` is reached from `Value`'s `Display`,
  which carries no dialect. The change here touches only text the SQLite dialect writes today.

## The page, rule by rule

Each rule: the sentence, where SQLite implements it, where Amphora does, the test.

| # | the page says | SQLite 3.54.0 | Amphora | test (crates/engine/tests/floating_point_doc.rs) |
|---|---|---|---|---|
| 1 | "This gives a storage range of -9223372036854775808 to +9223372036854775807, inclusive. Integers within this range are exact." | 64-bit integers (`sqlite3Atoi64`) | `Numeric::Integer(i64)` | `g10_integers_are_exact_over_the_64_bit_range` |
| 2 | "A binary-64 can also be 0.0 (and -0.0), positive and negative infinity and "NaN" or "Not-a-Number"." | doubles; text `Inf`, `-Inf` | `Numeric::Float(NonNan)`; `fp::render` | `g10_reals_hold_signed_zero_and_infinities` |
| 3 | "If you provide a "price" value of 47.49, that number will be represented in binary-64 as: 6683623321994527 × 2-47" and "The first number is closer to the desired value of 47.49, so that is the one that gets used." | `sqlite3AtoF`: keeps about 19 digits, misses the closer double on 32% of literals with 20+ significant digits (G9, r1132) | `numeric::str_to_f64`, `vdbe::affinity::try_for_float`: Rust's parse, nearest, every digit (G9, 67fd181a) | `g10_decimal_text_takes_the_closer_double`; G9's `g9_decimal_text_converts_to_the_correctly_rounded_double` |
| 4 | "But if you do this computation using binary-64 floating-point, the answer you get is 0.00000095367431640625" | IEEE subtraction | IEEE subtraction | `g10_subtraction_of_near_equal_reals_gives_the_documented_binary64` |
| 5 | "It would look at the 16th digit, see that it was zero and hence just truncate, then remove trailing zeros, and you are left with "47.49", which is the answer most people want." | `vdbeMemRenderNum` `"%!.*g"`, `sqlite3FpDecode` | `numeric::fp::fifteen_digits`, `render` | `g10_47_49_displays_as_47_49` |
| 6 | "Rounding to 17 significant digits or more results in text that, when coverted back into binary-64, gives the exact same value that we started with." | `sqlite3FpDecode(r, 17, ...)` | `numeric::fp::decode(r, 17, 20)` | `g10_displayed_text_converts_back_to_the_same_double` (every fixture double) |
| 7 | "If you round to 17 digits, you get 2.8781999999999997 instead." and "Converting the text "2.8781999999999997" back to binary-64 does go back to original value though." | the same | the same | `g10_product_that_needs_17_digits_displays_them` |
| 8 | "Round to 15 digits." "Convert the text result back to binary-64." "If the binary-64 value from the previous step is different, then: Redo step (a) with 17-digit rounding." | `sqlite3FpDecode`'s `iRound==17` shortcut: shorter only after `000` at digits 14-16 or `99` at 15-16 (differs, measured below) | `fp::render`: `fifteen_digits` (Rust's `{:.14e}` of the exact value, parsed back), else `decode(r, 17, 20)` | `g10_display_is_15_digits_when_they_convert_back_else_17`; the corpus |
| 9 | "Beginning with SQLite 3.52.0 (2026-03-06), solution (B) is available and is the default, though applications can choose to do back to solution (A)." (`sqlite3_db_config(db, SQLITE_DBCONFIG_FP_DIGITS, 15, 0)`) | `db->nFpDigit` | not implemented | none (successor) |
| 10 | "The ieee754 extension is not part of the amalgamation, but it is included by default in the CLI." | `ext/misc/ieee754.c`, CLI only | none (no `ieee754*` functions) | none: the library matches the amalgamation |
| 11 | "The decimal extension is not (currently) part of the SQLite amalgamation. However, it is included in the CLI." | `ext/misc/decimal.c`, CLI only | none (no `decimal*` functions) | none: as rule 10 |

`quote()` and JSON write a REAL through the same `fp::render` (`%!0.17g` in 3.54: the same digits,
infinity as `9.0e+999`), so they take the page's digits too. `printf()`'s `%g`, `%e`, `%f` keep their
own port (`functions/printf.rs`): printf.html governs them, not this page.

## The change (perf/numeric)

- `crates/engine/numeric/fp.rs`: `render` asks `fifteen_digits(r)` first (steps a and b: the 15
  digits Rust's `{:.14e}` rounds from the exact value, kept when Rust's parse reads back the same
  double), else `decode(r, 17, 20)` (step c). A tie at 15 digits can never convert back, so the
  tie rule of `{:.14e}` cannot show. The 3.54 shortcut and `fp10_convert2`, used only by it, are
  deleted (36 lines in, 107 out).
- `crates/engine/tests/fixtures/real_text.rs`: 107 of its 4,756 expected texts change, each where
  15 digits convert back (`numeric-scripts/refixture.py` on the lane's machine; every other line is
  still SQLite 3.54.0's own answer).
- `crates/engine/tests/floating_point_doc.rs`: the rule tests and the corpus writer.
- `bench/real_text_sqlite.py`: counts Amphora, SQLite and the page's algorithm over the corpus.

## Checks and the corpus

| run | head | what | result |
|---|---|---|---|
| r1251 | 924a3524 | `cargo clippy -p amphora-engine --all-targets --locked -- -D warnings` | clean |
| r1251 | 924a3524 | `--test floating_point_doc --test real_text` | 9 + 2 passed, 0 failed |
| r1251 | 924a3524 | `--lib -- numeric:: printf json::` | 230 passed, 1 failed: `test_decode_float` still held 3.54's 17-digit subnormals; 5a058fdd sets them to the page's (`9.93071948140906e-322`) |
| r1251 | 924a3524 | corpus (304,713 finite doubles: the fixture's, then 100,000 each of random bits, decimals of 0 to 15 places, products of two 2-place decimals) and `bench/real_text_sqlite.py`, SQLite 3.54.0 | below |
| r1256 | 2a42f9ac (merged with main) | clippy; `floating_point_doc real_text oracle_findings`; `--lib numeric:: printf json:: vdbe::value vdbe::affinity g9_` | clippy clean; 9 + 83 + 2 passed, 0 failed; lib 282 passed, 0 failed |

The corpus, by which path the page's algorithm takes for the double:

| agreement | page writes 15 digits (173,493) | page writes 17 digits (131,220) |
|---|---|---|
| Amphora = the page | 173,493 (100%) | 130,254 (99.264%) under the exact reading below |
| SQLite 3.54.0 = the page | 168,532 (97.141%) | 130,254 (99.264%) |
| Amphora = SQLite 3.54.0 | 168,532 (97.141%) | 131,220 (100%) |

The 966 17-digit doubles where both differ from the script's model are the page's ambiguity, not
a defect found: the script reads "round to 17 digits" as the exact value rounded to 18 digits, then
rounded at the 18th; SQLite's sqlite3FpDecode computes its 18 digits from a 64-bit product with an
approximate power of ten, and they differ in the 18th digit on those doubles
(`1.2194521461774974e-206`: SQLite and Amphora `...974`, the exact-18 reading `...975`). All texts
convert back to the same double either way. The page's only 17-digit example (2.8782) agrees with
both. Amphora keeps SQLite's digits there; the choice below covers it.

## Ada's choice: the page's 17 digits

- **The page's example, SQLite's digits (on main now).** 17-digit text is sqlite3FpDecode's: 18
  digits from its fixed-point product, then rounded at the 18th. It gives the page's
  `2.8781999999999997` and equals SQLite 3.54.0 on every 17-digit double of the corpus. Every text
  converts back to the same double, so no value changes; only the last printed digit can.
- **Correctly rounded 17 digits.** The exact value rounded once: `2.8781999999999996` (the shortest
  text, and what Python and Rust print). This contradicts the page's sentence for that example.

## Successor

1. `SQLITE_DBCONFIG_FP_DIGITS`: a connection setting (15 or 17) read by `fp::render`, its 15 path
   writing `round15` always, and the `%!.15g` layout (exponent past 14).
2. Read printf.html, lang_expr.html (CAST), datatype3.html (comparison of INTEGER with REAL) and the
   decimal/ieee754 extension pages; list their rules here with tests.
3. SQLite's C source at 3.54.0: confirm `vdbeMemRenderNum`, `sqlite3FpDecode`, `sqlite3AtoF` and
   the integer/real comparison (`sqlite3IntFloatCompare`) against the citations above.
4. Postgres dialect (coordinator, Ada 11-23): float8/float4 input correctly rounded (G9's
   converter), output by `extra_float_digits` (default 1, shortest round-trip), numeric exact, PG 19's
   special values and errors, routed through `Dialect` once the Postgres frontend renders values;
   tested against PostgreSQL 19.

## Commands

On adacachy2, needs `linux/amd64,cpus=4,mem=16g`, sync false, in `~/.cache/amphora-perf/numeric-remote`
at the commit named, with `CARGO_HOME=~/.cache/cargo-home-perf-numeric CARGO_NET_OFFLINE=true
CARGO_TARGET_DIR=~/.cache/amphora-perf-target/numeric CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0`:

    cargo clippy -p amphora-engine --all-targets --locked -- -D warnings
    cargo test -p amphora-engine --test floating_point_doc --test real_text -- --test-threads=4
    cargo test -p amphora-engine --lib -- numeric:: printf json:: --test-threads=4
    AMPHORA_REAL_TEXT_CORPUS=$HOME/.cache/amphora-g10/corpus.tsv cargo test -p amphora-engine --test floating_point_doc g10_real_text_corpus
    python3 -I bench/real_text_sqlite.py ~/.cache/amphora-g10/corpus.tsv

## The page, verbatim

https://www.sqlite.org/floatingpoint.html as fetched 2026-11-23 16:44Z ("This page was last updated
on 2026-05-20 11:35:23Z"; HTML sha256 913f9ae141b1b043...), its text with the markup removed
(superscripts flattened: "2-47" is 2^-47, "M × 2E" is M × 2^E):

    1. How SQLite Stores Numbers
    
    SQLite stores integer values in the 64-bit 
    twos-complement
    format¹.
    This gives a storage range of -9223372036854775808 to +9223372036854775807,
    inclusive.  Integers within this range are exact.
    
    So-called "REAL" or floating point values are stored in the
    IEEE 754
    Binary-64
    format¹.
    This gives a range of positive values between approximately
    1.7976931348623157e+308 and 4.9406564584124654e-324 with an equivalent
    range of negative values.  A binary-64 can also be 0.0 (and -0.0), positive
    and negative infinity and "NaN" or "Not-a-Number".  Floating point
    values are approximate.
    
    Pay close attention to the previous sentence:
    
    Floating point values are approximate.
    
    If you need an exact answer, you should not use binary-64 floating-point
    values, in SQLite or in any other product.  This is not an SQLite limitation.
    It is a mathematical limitation inherent in the design of floating-point numbers.
    
    —
    ¹
    Exception:  The R-Tree extension stores information as 32-bit floating
    point or integer values.
    
    1.1. Floating-Point Accuracy
    
    SQLite will preserve as many significant digits of a floating
    point value as it can.   But an IEEE 754 double only holds about
    15.95 significant digits.   Furthermore, SQLite makes no guarantees that
    the accuracy of computations on floating point values will even be
    this accurate, because no such guarantees are possible.
    Performing math on floating-point values introduces error.
    For example, consider what happens if you attempt to subtract two floating-point
    numbers of similar magnitude:
    
    1152693165.1106291898
    
    -1152693165.1106280772
    
    0.0000011126
    
    The result shown above (0.0000011126) is the correct answer.  But if you
    do this computation using binary-64 floating-point, the answer you get is
    0.00000095367431640625 - an error of about 14%.  If you do many similar
    computations as part of your program, the errors add up so that your final
    result might be completely meaningless.
    
    The error arises because only about 15.95 significant digits of
    each number are stored accurately using the IEEE 754 binary-64 format, and the
    first difference between the two numbers being subtracted is in the 16th digit.  
    
    1.2. Floating Point Numbers
    
    The binary-64 floating-point format uses 64 bits per number.  Hence there
    are 1.845e+19 different possible floating point values.  On the other hand
    there are infinitely many real numbers in the range of 
    1.7977e+308 and 4.9407e-324.  It follows then that binary-64 cannot possibly
    represent all possible real numbers within that range.  Approximations are
    required.
    
    An IEEE 754 floating-point value is an integer multiplied by a power
    of two:
    
    M × 2E
    
    The M value is the "mantissa" and E is the "exponent".  Both
    M and E are integers.
    
    For Binary-64, M is a 53-bit integer and E is an 11-bit integer that is
    offset so that represents a range of values between -1074 and +972, inclusive.
    
    (NB:  The usual description of IEEE 754 is more complex, and it is important
    to understand the added complexity if you really want to appreciate the details,
    merits, and limitations of IEEE 754.  However, the integer description shown
    here, while not exactly right, is easier to understand and is sufficient for
    the purposes of this article.)
    
    1.2.1. Unrepresentable numbers
    
    Not every decimal number with fewer than 16 significant digits can be
    represented exactly as a binary-64 number.  In fact, most decimal numbers
    with digits to the right of the decimal point lack an exact binary-64
    equivalent.  For example, if you have a database column that is intended
    to hold an item price in dollars and cents, the only cents value that
    can be exactly represented are 0.00, 0.25, 0.50, and 0.75.  Any other
    numbers to the right of the decimal point result in an approximation.
    If you provide a "price" value of 47.49, that number will be represented
    in binary-64 as:
    
    6683623321994527 × 2-47
    
    Which works out to be:
    
    47.49000000000000198951966012828052043914794921875
    
    That number is very close to 47.49, but it is not exact.  It is a little
    too big.  If we reduce M by one to 6683623321994526 so that we have the
    next smaller possible binary-64 value, we get:
    
    47.4899999999999948840923025272786617279052734375
    
    This second number is too small.
    The first number is closer to the desired value of 47.49, so that is the
    one that gets used.  But it is not exact.  Most decimal values work this
    way in IEEE 754.  Remember the key point we made above:
    
    Floating point values are approximate.
    
    If you remember nothing else about floating-point values, 
    please don't forget this one key idea.
    
    1.2.2. Is it close enough?
    
    The precision provided by IEEE 754 Binary-64 is sufficient for most computations.
    For example, if "47.49" represents a price and inflation is running
    at 2% per year, then the price is going up by about 0.0000000301 dollars per
    second.  The error in the recorded value of 47.49 represents about 66 nanoseconds
    worth of inflation.  So if the 47.49 price is exact
    when you enter it, then the effects of inflation will cause the true value to
    exactly equal the value actually stored
    (47.4900000000000019895196601282805204391479492187) in less than 
    one ten-millionth of a second.
    Surely that level of precision is sufficient for most purposes?
    
    1.3. Rounding Floating-Point Numbers For Display
    
    When converting a binary-64 number into text for display purposes
    (for example by calling sqlite3_column_text() or sqlite3_value_text()
    or by using the CAST operator in SQL), the value
    is normally rounded to a reasonable number of significant digits.  One
    rarely wants to see all 49 signficant digits of 
    6683623321994527×pow(2,-47). The first four digits, "47.49" in this case,
    is usually a better display option.
    
    There are also performance reasons for rounding.
    6683623321994527×pow(2,-47) can be converted into "47.49" relatively
    quickly, but generating the full 49-digit expansion requires a lot more
    computation.
    
    At what point should rounding occur?
    Rounding to 17 significant digits or more results in text that, when coverted
    back into binary-64, gives the exact same value that we started with.
    Rounding to 15 significant digits or less can often gives this exact round-trip result,
    but not always.
    On the other hand, rounding to 15 digits tends to give
    results that are more pleasing to human readers, whereas rounding to 17
    digits can result in long numbers with a lot of '0's or '9's in the middle.
    
    Prior to SQLite 3.52.0 (2026-03-06), if you tried to round
    6683623321994527×pow(2,-47) to 17 digits, SQLite would look at the first
    18 digits ("47.4900000000000019"), see that the last digit was 5 or more
    and hence round up to "47.490000000000002".  For that reason, in earlier versions
    of SQLite, rounding occurred at 15 digits.  It would look at the 16th digit,
    see that it was zero and hence just truncate, then remove trailing zeros,
    and you are left with "47.49", which is the answer most people want.
    And it turns out that converting text "47.49" back to binary-64 results
    in 6683623321994527×pow(2,-47), so the value does not change if you
    round-trip binary-64→text→binary-64.  All is well.
    
    But this does not work for every case.
    Consider, for example, the product of 1.23 and 2.34.
    The number 1.23 is:
    
    2769713770832855 × 2-51
    
       = 1.229999999999999982236431605997495353221893310546875
    
    And 2.34 is:
    
    5269211564023480 × 2-51
    
       = 2.339999999999999857891452847979962825775146484375
    
    If you do an exact multiplication of those two values you get:
    
    2.878199999999999783639736961049495926597557229698714817531408904915934954260592348873615264892578125
    
    Which cannot be represented in binary-64.  The floating point hardware
    in your CPU truncates this to:
    
    6481130223748880 × 2-51
    
       = 2.87819999999999964757080306299030780792236328125
    
    If you round that last value to 15 significant digits, you get 2.8782, which is
    the answer most people expect.  If you round to 17 digits, you get
    2.8781999999999997 instead.  However, 2.8782 and 2.8781999999999997 are not
    the same number in binary-64.
    
    2.8782 → 6481130223748881 × 2-51
    
    2.8781999999999997 →  6481130223748880 × 2-51
    
    The mantissa is different by one.  So converting the text "2.8782" back to
    binary-64 will not go back to the original product of 1.23×2.34.
    It will be very close, but not exact.  Converting the text "2.8781999999999997"
    back to binary-64 does go back to original value though.  And so in
    that sense, 2.8781999999999997 is a more accurate answer.
    
    1.3.1. Choosing The Number Of Floating-Point Digits To Display
    
    So there is a trade-off.  Do you (A) round to 15 digits, and provide answers
    that are closer to what humans expect to see, or do you (B) round to 17 digits and
    provide answers that sometimes have lots '0's or '9's after the decimal point
    but which will round-trip back to the original binary-64?
    
    With SQLite 3.51.2 and earlier, solution (A) was the only option.  Beginning
    with SQLite 3.52.0 (2026-03-06), solution (B) is available and is the default,
    though applications can choose to do back to solution (A).  In C-code, one can
    choose option (A) by running code like this:
    
    sqlite3_db_config(db, SQLITE_DBCONFIG_FP_DIGITS, 15, 0);
    
    Or in the CLI, one can do:
    
    .dbconfig fp_digits 15
    
    This change occurred because in version 3.52.0, the rounding algorithm
    was enhanced to work something like this:
    
     Round to 15 digits.
    
     Convert the text result back to binary-64.
    
     If the binary-64 value from the previous step is different, then:
    
      • Redo step (a) with 17-digit rounding.
    
     Rounding to 15 digits is usually enough.
    Step (c) seldom occurs.
    The text renderings with lots of '0's and '9's only appear
    in the unusual cases where they make a difference.  The reason this enhancement
    was made in version 3.52.0 was that the binary-64→text→binary-64 conversion
    routines were rewritten to use 64-bit integer arithmetic instead of floating-point
    computations, and so step (b) in the algorithm above became computationally 
    reasonable to perform.  The binary-64→text conversion in 3.52.0 is still
    faster than before, even with the addition of steps (b) and (c). 
    If you do not want the (negligible) overhead of step (b) and (c), just
    set SQLITE_DBCONFIG_FP_DIGITS to 15.
    
    2. Extensions For Dealing With Floating Point Numbers
    
    2.1. The ieee754.c Extension
    
    The ieee754 extension converts a floating point number between its
    binary-64 representation and the M×2E format.
    In other words in the expression:
    
    F = M × 2E
    
    The ieee754 extension converts between F and (M,E) and back again.
    
    The ieee754 extension is not part of the amalgamation, but it is included
    by default in the CLI.  If you want to include the ieee754 extension in your
    application, you will need to compile and load it separately.
    
    2.1.1. The ieee754() function
    
    The ieee754(F) SQL function takes a single floating-point argument
    as its input and returns a string that looks like this:
    
    'ieee754(M,E)'
    
    Except that the M and E are replaced by the mantissa and exponent of the
    floating point number.  For example:
    
    sqlite> .mode box
    sqlite> SELECT ieee754(47.49) AS x;
    ┌───────────────────────────────┐
    │               x               │
    ├───────────────────────────────┤
    │ ieee754(6683623321994527,-47) │
    └───────────────────────────────┘
    
    Going in the other direction, the 2-argument version of ieee754() takes
    the M and E values and converts them into the corresponding F value:
    
    sqlite> select ieee754(6683623321994527,-47) as x;
    ┌───────┐
    │   x   │
    ├───────┤
    │ 47.49 │
    └───────┘
    
    2.1.2. The ieee754_mantissa() and ieee754_exponent() functions
    
    The text output of the one-argument form of ieee754() is great for human
    readability, but it is awkward to use as part of a larger expression.  Hence
    the ieee754_mantissa() and ieee754_exponent() routines were added to return
    the M and E values corresponding to their single argument F
    value.
    For example:
    
    sqlite> .mode box
    sqlite> SELECT ieee754_mantissa(47.49) AS M, ieee754_exponent(47.49) AS E;
    ┌──────────────────┬─────┐
    │        M         │  E  │
    ├──────────────────┼─────┤
    │ 6683623321994527 │ -47 │
    └──────────────────┴─────┘
    
    2.1.3. The ieee754_from_blob() and ieee754_to_blob() functions
    
    The ieee754_to_blob(F) SQL function converts the floating point number F
    into an 8-byte BLOB that is the big-endian binary-64 encoding of that number.
    The ieee754_from_blob(B) function goes the other way, converting an 8-byte
    blob into the floating-point value that the binary-64 encoding represents.
    
    So, for example, if you read
    on
    Wikipedia that the encoding for the minimum positive binary-64 value is
    0x0000000000000001, then you can find the corresponding floating point value
    like this:
    
    sqlite> .mode box
    sqlite> SELECT ieee754_from_blob(x'0000000000000001') AS F;
    ┌───────────────────────┐
    │           F           │
    ├───────────────────────┤
    │ 4.94065645841247e-324 │
    └───────────────────────┘
    
    Or go the other way:
    
    sqlite> .mode box
    sqlite> SELECT quote(ieee754_to_blob(4.94065645841247e-324)) AS binary-64;
    ┌─────────────────────┐
    │      binary-64      │
    ├─────────────────────┤
    │ X'0000000000000001' │
    └─────────────────────┘
    
    2.2. The decimal.c Extension
    
    The decimal extension provides arbitrary-precision decimal arithmetic on
    numbers stored as text strings.  Because the numbers are stored to arbitrary
    precision and as text, no approximations are needed.  Computations can be
    done exactly.
    
    The decimal extension is not (currently) part of the SQLite amalgamation.
    However, it is included in the CLI.  The source code to this extension
    can be found at
    ext/misc/decimal.c.
    
    The decimal extension supports the following SQL functions and collating sequences:
    
    2.2.1. The decimal_add(A,B), decimal_sub(A,B), and decimal_mul(A,B) functions
    
    These functions compute the sum, difference, and product, respectively, of
    the two inputs A and B and return the result as text.  The inputs can be either
    decimal text or numeric values.  Numeric inputs are converted into decimal
    text before the computation is performed.
    
    There is no "decimal_div(A,B)" function because division does not always
    have a finite-length decimal result.
    
    2.2.2. The decimal_pow2(N) function
    
    The decimal_pow2(N) function computes the exact decimal representation of
    the N-th power of 2.  N must be an integer between -20000 and +20000.
    
    This function can be slow and can use a lot of memory for large values of N.
    
    2.2.3. The decimal(X) and decimal_exp(X) functions
    
    The decimal(X) and decimal_exp(X) generate a decimal representation for input X.
    The input X can be an integer or a floating point number, or a text decimal.
    The decimal_exp(X) function returns the result in exponential notation (with a "e+NN"
    at the end) and decimal(X) returns a pure decimal (without the "e+NN").
    
    If the input
    X is a floating point value, it is expanded to its exact decimal equivalent.  For
    example:
    
    sqlite> .mode qbox
    sqlite> select decimal(47.49);
    ┌──────────────────────────────────────────────────────┐
    │                    decimal(47.49)                    │
    ├──────────────────────────────────────────────────────┤
    │ '47.49000000000000198951966012828052043914794921875' │
    └──────────────────────────────────────────────────────┘
    
    2.2.4. The decimal_cmp(X) function
    
    The decimal_cmp(A,B) function compares two decimal values A and B.  The result will
    be negative, zero, or positive if A is less than, equal to, or greater than B,
    respectively.
    
    2.2.5. The decimal_sum(X) aggregate function
    
    The decimal_sum(X) function is an aggregate, like the built-in
    total() aggregate function, except that decimal_sum() computes its result
    to arbitrary precision and is therefore precise.
    
    2.2.6. The decimal collating sequence
    
    The decimal extension provides the "decimal" collating sequence
    that compares decimal text strings in numeric order.
    
    This page was last updated on 2026-05-20 11:35:23Z 
