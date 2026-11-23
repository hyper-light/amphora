#!/usr/bin/env python3
"""Lift bison's tables for PostgreSQL 19's gram.y into Rust constants (docs/design/postgres.md
section 3; section 11's unit 5). Reads the gram.c that `bison -d` writes (PostgreSQL's flags,
src/backend/parser/Makefile) for the parse tables, and the -t run's gram.c for the symbol names
(yytname and yyrline, which only a debug build declares); refuses if the two runs' parse tables
differ. Each rule's gram.y line comes from yyrline, and each rule's action from gram.c's reduce switch:
its C text as bison writes it (gram.y's text with $$, $n and @n rewritten), hashed FNV-1a 64, so a
ported Rust action can name the hash of the text it was ported from (section 3).
Prints crates/pg-parser/src/gram_tables.rs.

usage: postgres-grammar.py <gram.c> <gram.c of bison -t> <bison version> <sha256 of gram.y>"""
import re
import sys

TYPES = {"yytype_int8": "i8", "yytype_uint8": "u8", "yytype_int16": "i16", "yytype_uint16": "u16", "int": "i32", "yytype_int32": "i32"}
TABLES = ["yytranslate", "yypact", "yydefact", "yypgoto", "yydefgoto", "yytable", "yycheck", "yystos", "yyr1", "yyr2"]
DEFINES = ["YYFINAL", "YYLAST", "YYNTOKENS", "YYNNTS", "YYNRULES", "YYNSTATES", "YYMAXUTOK"]


def table(src: str, name: str):
    m = re.search(r"static const (\w+) " + name + r"\[\] =\s*\{(.*?)\};", src, re.S)
    if not m:
        sys.exit(f"{name} not found")
    if m.group(1) not in TYPES:
        sys.exit(f"{name}: C type {m.group(1)} has no Rust type here")
    return TYPES[m.group(1)], [int(v) for v in re.findall(r"-?\d+", m.group(2))]


def define(src: str, name: str) -> int:
    m = re.search(r"^#define " + name + r"\s+\(?(-?\d+)\)?\s*$", src, re.M)
    if not m:
        sys.exit(f"{name} not found")
    return int(m.group(1))


def ninf_test(src: str, macro: str, ninf: str):
    """bison writes `((Yyn) == NINF)` when NINF can occur in the table, else `0`: the value to test, or None."""
    m = re.search(r"^#define " + macro + r"\(Yyn\) \\\n\s*(.*?)\s*$", src, re.M)
    if not m:
        sys.exit(f"{macro} not found")
    if m.group(1) == "0":
        return None
    if m.group(1) != f"((Yyn) == {ninf})":
        sys.exit(f"{macro}: unexpected body {m.group(1)}")
    return define(src, ninf)


def names(src: str):
    m = re.search(r"static const char \*const yytname\[\] =\s*\{(.*?)\};", src, re.S)
    if not m:
        sys.exit("yytname not found")
    out = []
    for lit in re.findall(r'"((?:[^"\\]|\\.)*)"', m.group(1)):
        out.append(re.sub(r"\\(.)", r"\1", lit))
    return out


def fnv1a(data: bytes) -> int:
    h = 0xCBF29CE484222325
    for b in data:
        h = ((h ^ b) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


def actions(src: str, nrules: int, lhs_names):
    """Each rule's action hash from yyparse's `switch (yyn)`: None for a rule with no action."""
    start = src.find("  switch (yyn)\n")
    end = src.find("      default: break;", start)
    if start < 0 or end < 0:
        sys.exit("the reduce switch not found")
    out = [None] * (nrules + 1)
    found = 0
    for m in re.finditer(r'^  case (\d+): /\* (.*?): .*? \*/\n#line \d+ "gram\.y"\n(.*?)\n#line \d+ "gram\.c"\n    break;$', src[start:end], re.M | re.S):
        rule, lhs, text = int(m.group(1)), m.group(2), m.group(3)
        if not 0 < rule <= nrules or out[rule] is not None:
            sys.exit(f"case {rule}: outside the rules or seen twice")
        if lhs_names[rule] != lhs:
            sys.exit(f"case {rule}: its comment names {lhs}, yyr1 names {lhs_names[rule]}")
        out[rule] = fnv1a(text.encode("utf-8"))
        found += 1
    if found != len(re.findall(r"^  case \d+: ", src[start:end], re.M)):
        sys.exit("a case of the reduce switch didn't read as one action")
    return out


def rust_str(s: str) -> str:
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def main() -> None:
    plain_path, named_path, version, digest = sys.argv[1:5]
    plain = open(plain_path, encoding="utf-8").read()
    named = open(named_path, encoding="utf-8").read()
    for t in TABLES:
        if table(plain, t) != table(named, t):
            sys.exit(f"{t} differs between bison -d and bison -d -t")
    for d in DEFINES:
        if define(plain, d) != define(named, d):
            sys.exit(f"{d} differs between bison -d and bison -d -t")
    d = {name: define(plain, name) for name in DEFINES}
    pact_ninf = ninf_test(plain, "yypact_value_is_default", "YYPACT_NINF")
    table_ninf = ninf_test(plain, "yytable_value_is_error", "YYTABLE_NINF")
    tname = names(named)
    if len(tname) != d["YYNTOKENS"] + d["YYNNTS"]:
        sys.exit(f"yytname: {len(tname)} names for {d['YYNTOKENS'] + d['YYNNTS']} symbols")
    rline_type, rline = table(named, "yyrline")
    if len(rline) != d["YYNRULES"] + 1:
        sys.exit(f"yyrline: {len(rline)} lines for {d['YYNRULES']} rules")
    r1 = table(plain, "yyr1")[1]
    hashes = actions(plain, d["YYNRULES"], [tname[x] for x in r1])
    out = [
        f"//! bison {version}'s tables for vendor/postgres/src/backend/parser/gram.y (sha256 {digest}),",
        "//! generated by scripts/postgres-grammar.sh as PostgreSQL builds it (`bison -d`), with the symbol",
        "//! names of a `-t` run whose tables are the same. Not edited by hand: regenerate.",
        "",
        f"pub const GRAM_Y_SHA256: &str = \"{digest}\";",
        "/// YYFINAL: the accepting state.",
        f"pub const FINAL: i32 = {d['YYFINAL']};",
        "/// YYLAST: the last index of TABLE and CHECK.",
        f"pub const LAST: i32 = {d['YYLAST']};",
        "/// YYNTOKENS: terminal symbols; nonterminals are numbered from here.",
        f"pub const NTOKENS: i32 = {d['YYNTOKENS']};",
        f"pub const NNTS: i32 = {d['YYNNTS']};",
        f"pub const NRULES: i32 = {d['YYNRULES']};",
        f"pub const NSTATES: i32 = {d['YYNSTATES']};",
        "/// YYMAXUTOK: the largest token number TRANSLATE maps.",
        f"pub const MAXUTOK: i32 = {d['YYMAXUTOK']};",
        "/// yypact_value_is_default's value (YYPACT_NINF): the state's action is its default one.",
        f"pub const PACT_DEFAULT: Option<i32> = {'None' if pact_ninf is None else f'Some({pact_ninf})'};",
        "/// yytable_value_is_error's value (YYTABLE_NINF): a syntax error.",
        f"pub const TABLE_ERROR: Option<i32> = {'None' if table_ninf is None else f'Some({table_ninf})'};",
        f"/// The `stmt` nonterminal: each alternative names a statement kind.",
        f"pub const STMT: i32 = {tname.index('stmt')};",
    ]
    for t in TABLES:
        ty, values = table(plain, t)
        out.append(f"pub static {t[2:].upper()}: [{ty}; {len(values)}] = [{', '.join(str(v) for v in values)}];")
    out.append("/// yyrline: the gram.y line each rule is written on (rule 0 is bison's own).")
    out.append(f"pub static RLINE: [{rline_type}; {len(rline)}] = [{', '.join(str(v) for v in rline)}];")
    out.append("/// Each rule's action: FNV-1a 64 of its C text in gram.c's reduce switch, None for a rule without one.")
    out.append(f"pub static ACTION_HASH: [Option<u64>; {len(hashes)}] = [{', '.join('None' if h is None else f'Some(0x{h:016x})' for h in hashes)}];")
    out.append("/// yytname: each symbol's name, terminals first.")
    out.append(f"pub static TNAME: [&str; {len(tname)}] = [{', '.join(rust_str(n) for n in tname)}];")
    print("\n".join(out))


main()
