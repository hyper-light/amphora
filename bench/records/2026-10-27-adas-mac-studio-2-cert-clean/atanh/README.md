# atanh against each platform's C library

The evidence for decision 2 of `docs/design/semantics.md`: SQLite's atanh is its C library's, so
Amphora's `cmath::atanh` (`crates/engine/vdbe/value.rs`) computes macOS's on Apple platforms and the
correctly rounded value elsewhere. Run 2026-10-27 on adacachy (glibc 2.44, Linux 7.2.9 x86_64) and
on adas-mac-studio-2 (macOS 26.2, build 25C56, Apple silicon), from these files, as they are.

- `imp.rs`: the two computations, copied from `value.rs` (the Apple one and the double-double one).
- `main.rs`: each computation against the platform's atanh, called from C, on 3,000,019 arguments:
  a quarter each uniform in (-1, 1), uniform in (-0.2, 0.2), uniform within a binade from 2^-61
  to 2^-1, and 1 - 2^-k for k uniform in [1, 52], each signed at random; and ±0.5, 0.1, 0.25, 0.3,
  ±0, ±1, 2, both thresholds, 3 - 2√2 and the double above it, ±(1 - 2^-53), 1e-308, the smallest
  subnormal and NaN.
  `rustc -O --edition 2021 main.rs -o check && ./check 3000000`.
- `forms.py ref`: the platform's atanh against a 60-digit computation (Python's decimal; the series
  below 1e-4, where 60 digits can't hold 1 + 2x) and against four formulas on the platform's log1p
  and log, for 70,000 random arguments.
- `gen.py`: the same for every REAL the oracles' generator makes in (-1, 1): its numeric literals,
  their negations and one +, -, *, / of two (159 values).
- `apple.py`: macOS's atanh against ln_1p(2|x| / (1 - |x|)) / 2 given x's sign, with x itself
  below thresholds from 2^-26 to 2^-29, on 450,000 arguments.

| host | correctly rounded = libc | Apple's computation = libc | libc = 60 digits (70,000) | libc = 60 digits (generator's 159) |
|---|---:|---:|---:|---:|
| adacachy, glibc 2.44 | 3,000,019 of 3,000,019 | 2,544,037 | 100% | 159 |
| adas-mac-studio-2, macOS 26.2 | 2,560,307 | 3,000,019 of 3,000,019 | 83.84% | 146 |

macOS's atanh is the Apple computation with the threshold at 2^-27 and at no other tried
(`macos-apple.txt`): 0 differences of 450,000 there, 648 to 1,305 at the others. The correctly
rounded value is an ulp from macOS's at 0.25 and 0.3 among the generator's REALs, and at 13 of
them in all (`macos-gen.txt`).
