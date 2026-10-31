#!/usr/bin/env python3
"""The inventory's test (G2 step 1: "the inventory against the docs' anchors and function lists:
nothing documented missing"). Reads each list SQLite's documentation keeps of its own language,
apart from how inventory.py reads the pages, and checks every item is in features.json:

- lang.html's topics (every statement and clause page), each mapped below to the entries that
  cover it: a topic the map doesn't know fails, as does a mapped entry the inventory lacks;
- syntaxdiagrams.html: every syntax production;
- each function page's own index: the links of lang_corefunc.html, lang_aggfunc.html and
  lang_mathfunc.html, the signatures of lang_datefunc.html, the headings of json1.html and the
  built-in list of windowfunctions.html;
- pragma.html's list of PRAGMAs; lang_keywords.html's keywords, against every upper-case word of the
  syntax diagrams; the operators of lang_expr.html's precedence table and its NOT forms;
- and SQLite itself (the sqlite3 module's build): its built-in functions (pragma_function_list) and
  PRAGMAs (pragma_pragma_list), each either in the inventory or named below as undocumented.

usage: check_inventory.py <sqlite-doc directory> <features.json>
  prints each source's coverage; exits 1 if anything documented is missing.
"""
import html
import json
import os
import re
import sqlite3
import sys

DOC, FEATURES = sys.argv[1], sys.argv[2]
inv = json.load(open(FEATURES))["features"]
ids = {f["id"] for f in inv}
by_kind = {}
for f in inv:
    by_kind.setdefault(f["kind"], set()).add(f["name"])
functions = set().union(*(v for k, v in by_kind.items() if k.startswith("function.")))


def page(name):
    with open(os.path.join(DOC, name), encoding="utf-8", errors="replace") as f:
        return f.read()


def plain(text):
    return re.sub(r"\s+", " ", html.unescape(re.sub(r"<[^>]+>", " ", text)))


failures = []
report = []


def check(source, documented, present):
    documented = sorted(set(documented))
    missing = [d for d in documented if not present(d)]
    report.append((source, len(documented), len(documented) - len(missing)))
    for m in missing:
        failures.append(f"{source}: {m}")


# lang.html's topics: each maps to the entries that cover it.
TOPICS = {
    "aggregate functions": ["kind:function.aggregate"], "ALTER TABLE": ["syntax:alter-table-stmt"], "ANALYZE": ["syntax:analyze-stmt"],
    "ATTACH DATABASE": ["syntax:attach-stmt"], "BEGIN TRANSACTION": ["syntax:begin-stmt"], "comment": ["syntax:comment-syntax"],
    "COMMIT TRANSACTION": ["syntax:commit-stmt"], "core functions": ["kind:function.core"], "CREATE INDEX": ["syntax:create-index-stmt"],
    "CREATE TABLE": ["syntax:create-table-stmt", "syntax:column-def", "syntax:table-constraint"], "CREATE TRIGGER": ["syntax:create-trigger-stmt"],
    "CREATE VIEW": ["syntax:create-view-stmt"], "CREATE VIRTUAL TABLE": ["syntax:create-virtual-table-stmt"], "date and time functions": ["kind:function.datetime", "kind:datetime.modifier"],
    "DELETE": ["syntax:delete-stmt"], "DETACH DATABASE": ["syntax:detach-stmt"], "DROP INDEX": ["syntax:drop-index-stmt"], "DROP TABLE": ["syntax:drop-table-stmt"],
    "DROP TRIGGER": ["syntax:drop-trigger-stmt"], "DROP VIEW": ["syntax:drop-view-stmt"], "END TRANSACTION": ["syntax:commit-stmt"],
    "EXPLAIN": ["syntax:sql-stmt"], "expression": ["syntax:expr", "kind:operator", "kind:parameter", "kind:expression"], "INDEXED BY": ["syntax:qualified-table-name"],
    "INSERT": ["syntax:insert-stmt"], "keywords": ["kind:keyword"], "SQL keywords": ["kind:keyword"], "math functions": ["kind:function.math"],
    "ON CONFLICT clause": ["syntax:conflict-clause"], "REINDEX": ["syntax:reindex-stmt"], "RELEASE SAVEPOINT": ["syntax:release-stmt"],
    "REPLACE": ["syntax:insert-stmt"], "RETURNING clause": ["syntax:returning-clause"], "ROLLBACK TRANSACTION": ["syntax:rollback-stmt"],
    "SAVEPOINT": ["syntax:savepoint-stmt"], "SELECT": ["syntax:select-stmt"], "UPDATE": ["syntax:update-stmt"], "UPSERT": ["syntax:upsert-clause"],
    "VACUUM": ["syntax:vacuum-stmt"], "WITH clause": ["syntax:with-clause", "syntax:common-table-expression", "syntax:recursive-cte"],
    "PRAGMA": ["syntax:pragma-stmt", "kind:pragma"],
}
lang = page("lang.html")
topics = [html.unescape(t).strip() for t in re.findall(r"<li><a href=['\"](?:lang_[a-z_]+\.html|pragma\.html#syntax)['\"]>([^<]+)</a>", lang)]
if not topics:
    failures.append("lang.html: no topics found (the page changed shape)")


def covered(topic):
    targets = TOPICS.get(topic)
    if not targets:
        return False
    return all((t[5:] in by_kind) if t.startswith("kind:") else (t in ids) for t in targets)


check("lang.html topics", topics, covered)

# syntaxdiagrams.html: every production.
prods = [a for a in re.findall(r'<a name="([a-z][a-z-]*)"', page("syntaxdiagrams.html"))]
check("syntaxdiagrams.html productions", prods, lambda p: f"syntax:{p}" in ids)

# The function pages' own indexes.
for pg in ["lang_corefunc.html", "lang_aggfunc.html", "lang_mathfunc.html"]:
    names = re.findall(rf"href=['\"]{re.escape(pg)}#[a-z_0-9]+['\"]>\s*([a-z_0-9]+)\(", page(pg))
    check(f"{pg} index", names, lambda n: n in functions)
dt_names = re.findall(r"\b([a-z_]+)\(\s*(?:time-value|format)", plain(page("lang_datefunc.html")))
check("lang_datefunc.html signatures", dt_names, lambda n: n in functions)
js = page("json1.html")
js_names = []
for h in re.findall(r"<h[1-4][^>]*>(.*?)</h[1-4]>", js, re.S):
    js_names += re.findall(r"\b(jsonb?(?:_[a-z_]+)?)\(\)", plain(h))
check("json1.html headings", js_names, lambda n: n in functions)
w = page("windowfunctions.html")
win = re.findall(r"<dt><p><b>([a-z_]+)\(", w[w.find('name="biwinfunc"'):])
check("windowfunctions.html built-ins", win, lambda n: n in by_kind.get("function.window", set()))

# PRAGMAs.
pragmas = re.findall(r"href=['\"]pragma\.html#pragma_([a-z_0-9]+)['\"]", page("pragma.html"))
check("pragma.html list", pragmas, lambda n: n in by_kind.get("pragma", set()))

# Keywords: the page's list, and every upper-case word the syntax diagrams use is one of them.
kw_page = re.findall(r"<li>([A-Z_]+)</li>", page("lang_keywords.html"))
check("lang_keywords.html list", kw_page, lambda k: k in by_kind.get("keyword", set()))
diagram_words = set()
for f in inv:
    if f["kind"] == "syntax":
        diagram_words |= set(f.get("keywords", []))
check("syntax diagrams' words", diagram_words, lambda k: k in by_kind.get("keyword", set()) or k in by_kind.get("word", set()))

# Operators: the precedence table's cells and the NOT forms.
ex = page("lang_expr.html")
tab = ex[ex.find("<table", ex.find("SQLite understands these operators")):]
tab = tab[:tab.find("</table>")]
words = []
for c in re.findall(r"<td[^>]*optab[^>]*>(.*?)</td>", tab, re.S):
    c = re.sub(r"<sup>.*?</sup>", "", c, flags=re.S)
    c = re.sub(r"<small[^>]*>.*?</small>", " ", c, flags=re.S)
    c = re.sub(r"\(collation-name\)|\[escape-character-expr\]", " ", html.unescape(re.sub(r"<[^>]+>", " ", c)))
    words += c.split()
ops = by_kind.get("operator", set())
op_words = set()
for o in ops:
    op_words |= set(o.replace("(unary)", "").split())
check("lang_expr.html operator table", words, lambda w_: w_ in op_words)
nots = re.search(r"Each keyword in \(([A-Z ]+)\) may be prefixed", plain(ex))
check("lang_expr.html NOT forms", [f"NOT {k}" for k in (nots.group(1).split() if nots else [])], lambda o: o in ops)

# SQLite itself: its built-in functions and PRAGMAs, documented or named here as not.
UNDOCUMENTED = {
    # Internal: SQLite's own schema rewriting and testing, not part of the language.
    "sqlite_rename_column", "sqlite_rename_table", "sqlite_rename_test", "sqlite_rename_quotefix", "sqlite_drop_column", "sqlite_table_type",
    # Extension modules' functions (FTS3/4/5, R-Tree, Geopoly, the dbstat and stmt modules), documented with their modules.
    "fts3_tokenizer", "fts5", "fts5_source_id", "fts5_locale", "fts5_insttoken", "fts5_get_locale", "matchinfo", "offsets", "optimize", "snippet", "highlight", "bm25",
    "rtreenode", "rtreedepth", "rtreecheck",
    # SQLite's test hooks, and subtype(), a debugging function the language pages don't name.
    "sqlite_log", "unknown", "subtype",
}
con = sqlite3.connect(":memory:")
fl = {r[0] for r in con.execute("SELECT name FROM pragma_function_list WHERE builtin")}
check("SQLite's function_list (build " + sqlite3.sqlite_version + ")", fl, lambda n: n in functions or n in UNDOCUMENTED or n in ops or n.upper() in by_kind.get("keyword", set()))
pl = {r[0] for r in con.execute("SELECT name FROM pragma_pragma_list")}
check("SQLite's pragma_list (build " + sqlite3.sqlite_version + ")", pl, lambda n: n in by_kind.get("pragma", set()))

width = max(len(s) for s, _, _ in report)
for source, n, ok in report:
    print(f"{source:<{width}}  {ok:4d} of {n:4d}  {'100%' if n == ok else f'{100 * ok / n:.1f}%'}")
if failures:
    print("\nmissing from the inventory:")
    for f in failures:
        print("  " + f)
    sys.exit(1)
print(f"\nnothing documented missing: {len(inv)} entries")
