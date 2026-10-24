# Semantics: where Amphora's answers depart from SQLite's

Amphora answers as SQLite 3.54.0 answers (the oracles' reference, `bench/oracles`; G2 step 3), except
where a decision here says otherwise. Each one is recorded with its sources, its evidence and what
would falsify it (CLAUDE.md §5). LIKE and GLOB on a BLOB are decided in `planner.md` (decision 3).

## 1. TEXT is well-formed UTF-8

**Decision.** A TEXT value always holds well-formed UTF-8 (`types.rs`, `Text` holds a `str`). Bytes
become text when a BLOB is cast to TEXT or read where text is wanted (`||`, upper(), a JSON
function), and when char() is given a codepoint it can't write (a surrogate, U+D800 to U+DFFF). Each
ill-formed sequence among them is replaced by U+FFFD, substituting maximal subparts (Rust's
`String::from_utf8_lossy`). SQLite keeps the bytes. `CAST(x'ff' AS TEXT)` is the one byte FF there
and the three bytes of U+FFFD here; `char(55296)` is `ED A0 80` there and U+FFFD here.

**Why.** Every text operation in the engine reads a `str`. Holding arbitrary bytes would put a
validity check, or undefined behaviour, behind each one. And SQLite promises nothing for such text,
so none of its answers is the right one to match.

**Sources.** SQLite's "Invalid UTF Policy" (sqlite.org/invalidutf.html, updated 2025-05-31):
"With regard to invalid UTF, SQLite follows a policy of Garbage-In, Garbage-Out (GIGO)"; "If you
insert invalid UTF into an SQLite database, then SQLite makes no guarantees about what text you
might get back out". Its only promise is that invalid UTF causes no memory error and no crash. The
replacement follows the Unicode Standard, chapter 3, section 3.9, "U+FFFD Substitution of Maximal
Subparts".

**Evidence.** The oracles' reference answers hold no ill-formed text: `bench/src/oracle/engines.rs`
skips an answer that does, under SQLite's GIGO policy. Their BLOB literals are well-formed UTF-8
(`synth.rs`). Finding diff-becba9a65574ed81 (`jsonb_array(CAST(x'ff' AS TEXT))`) is filed under
this decision (`bench/oracles/findings.md`). `char_reads_its_integers_whole`
(`crates/engine/tests/oracle_findings.rs`) checks the surrogate.

**Falsified by.** An application that stores ill-formed UTF-8 as TEXT and needs its bytes back. It
should store a BLOB instead.
