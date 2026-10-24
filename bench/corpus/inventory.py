#!/usr/bin/env python3
"""The SQLite language, inventoried from SQLite's own documentation (G2 step 1, docs/research and
docs/requirements.toml AC-T1.2.1): every syntax production with the keywords and sub-productions
its diagram holds, every documented function with its signatures, every PRAGMA, the operators,
the type affinities and collations, and the keywords. The corpus covers what this lists; the
probes (probes.py) say which engines support each entry.

usage: python3 inventory.py <sqlite-doc directory> <out.json>
  (the directory of sqlite.org's documentation zip, e.g. sqlite-doc-3540100)
"""
import html
import json
import os
import re
import sys

DOC, OUT = sys.argv[1], sys.argv[2]
VERSION = re.search(r"sqlite-doc-(\d+)", os.path.abspath(DOC))
URL = "https://www.sqlite.org/"


def raw(page):
    with open(os.path.join(DOC, page), encoding="utf-8", errors="replace") as f:
        return f.read()


def text_with_anchors(page):
    """The page's visible text, each `<a name=...>` kept as `@@name@@` so entries can be cut at
    their anchors."""
    t = raw(page)
    t = re.sub(r'<a name="([^"]+)"></a>', r" @@\1@@ ", t)
    t = re.sub(r"<(script|style)[^>]*>.*?</\1>", " ", t, flags=re.S)
    t = re.sub(r"<[^>]+>", "", t)
    return re.sub(r"\s+", " ", html.unescape(t))


features = []


def add(kind, name, page, anchor=None, **detail):
    features.append({"id": f"{kind}:{name}", "kind": kind, "name": name, "doc": URL + page + (f"#{anchor}" if anchor else ""), **detail})


# 1. Syntax productions: each diagram's labels, keywords (upper case) apart from sub-productions.
syn = os.path.join(DOC, "syntax")
for f in sorted(os.listdir(syn)):
    if not f.endswith(".html"):
        continue
    labels = [html.unescape(x) for x in re.findall(r"<text[^>]*>([^<]+)</text>", raw(os.path.join("syntax", f)))]
    keywords = sorted({x for x in labels if re.fullmatch(r"[A-Z][A-Z_]*", x)})
    subs = sorted({x for x in labels if re.fullmatch(r"[a-z][a-z0-9-]*", x)})
    punct = sorted({x for x in labels if not re.fullmatch(r"[A-Za-z][A-Za-z0-9_-]*", x)})
    add("syntax", f[:-5], f"syntax/{f}", keywords=keywords, productions=subs, punctuation=punct)

# 2. Functions: each entry's signatures, cut at the page's anchors.
PAGES = {
    "core": "lang_corefunc.html",
    "aggregate": "lang_aggfunc.html",
    "window": "windowfunctions.html",
    "datetime": "lang_datefunc.html",
    "math": "lang_mathfunc.html",
    "json": "json1.html",
}
SIG = re.compile(r"\b([a-z_][a-z_0-9]*)\(((?:[^()]|\([^()]*\))*)\)")
for group, page in PAGES.items():
    t = text_with_anchors(page)
    seen = {}
    if group in ("core", "math"):
        # Each function's entry starts at its anchor with a run of signatures, aliases among
        # them (substr and substring, iif and if, log and log10), before its description.
        for anchor, body in re.findall(r"@@([a-z_0-9]+)@@([^@]*)", t):
            pos = 0
            body = body.lstrip()
            while True:
                m = SIG.match(body, pos)
                if not m:
                    break
                seen.setdefault(m.group(1), set()).add(f"{m.group(1)}({m.group(2).strip()})")
                pos = m.end()
                while pos < len(body) and body[pos] == " ":
                    pos += 1
    elif group == "json":
        # Signatures only: the page's examples (json_each(user.phone), a table's column list) aren't.
        for name, args in SIG.findall(t):
            if name.startswith("json") and re.fullmatch(r"[A-Za-z0-9_, ]*(?:\.\.\.)?[A-Za-z0-9_, ]*", args.strip()):
                seen.setdefault(name, set()).add(f"{name}({args.strip()})")
    elif group == "aggregate":
        for name in ["avg", "count", "group_concat", "string_agg", "max", "min", "sum", "total", "median", "percentile", "percentile_cont", "percentile_disc"]:
            for n, args in SIG.findall(t):
                if n == name:
                    seen.setdefault(name, set()).add(f"{name}({args.strip()})")
    elif group == "window":
        for name in ["row_number", "rank", "dense_rank", "percent_rank", "cume_dist", "ntile", "lag", "lead", "first_value", "last_value", "nth_value"]:
            for n, args in SIG.findall(t):
                if n == name:
                    seen.setdefault(name, set()).add(f"{name}({args.strip()})")
    elif group == "datetime":
        for name in ["date", "time", "datetime", "julianday", "unixepoch", "strftime", "timediff"]:
            for n, args in SIG.findall(t):
                if n == name:
                    seen.setdefault(name, set()).add(f"{name}({args.strip()})")
    for name in sorted(seen):
        add(f"function.{group}", name, page, name, signatures=sorted(seen[name])[:8])

# Date and time modifiers: their own list.
dt = text_with_anchors("lang_datefunc.html")
mods = sorted(set(re.findall(r"\b(NNN (?:days|hours|minutes|seconds|months|years)|ceiling|floor|start of (?:month|year|day)|weekday N|unixepoch|julianday|auto|localtime|utc|subsec|subsecond|[+-]HH:MM(?::SS(?:\.SSS)?)?|[+-]YYYY-MM-DD(?: HH:MM(?::SS(?:\.SSS)?)?)?)\b", dt)))
for m in mods:
    add("datetime.modifier", m, "lang_datefunc.html", "dtmods")

# 3. PRAGMAs.
for p in sorted(set(re.findall(r'<a name="pragma_([a-z_0-9]+)"></a>', raw("pragma.html")))):
    add("pragma", p, "pragma.html", f"pragma_{p}")

# 4. Operators: every cell of the expression page's precedence table (unary ones apart from binary
# ones, as it lists them), the NOT forms its text allows ("Each keyword in (BETWEEN IN GLOB LIKE
# MATCH REGEXP) may be prefixed by NOT"), and the operators and forms it gives sections of their own.
ex_raw = raw("lang_expr.html")
table = ex_raw[ex_raw.find("<table", ex_raw.find("SQLite understands these operators")):]
table = table[:table.find("</table>")]
cells = []
for c in re.findall(r"<td[^>]*class=['\"]?optab['\"]?[^>]*>(.*?)</td>", table, re.S):
    c = re.sub(r"<sup>.*?</sup>", "", c, flags=re.S)
    c = re.sub(r"<small[^>]*>\s*\[expr\]\s*</small>", "[expr]", c)
    c = re.sub(r"<br\s*/?>", "\n", c)
    cells.append(html.unescape(re.sub(r"<[^>]+>", "", c)))
if not cells:
    raise SystemExit("lang_expr.html's operator table wasn't found")
# Each cell holds operators separated by spaces or line breaks; the multi-word ones are matched
# first, longest first.
MULTI = ["[expr] BETWEEN [expr] AND [expr]", "IS NOT DISTINCT FROM", "IS DISTINCT FROM", "IS NOT", "[expr] COLLATE (collation-name)",
         "[expr] ESCAPE [escape-character-expr]", "[expr] ISNULL", "[expr] NOTNULL", "[expr] NOT NULL", "NOT [expr]", "~ [expr]", "+ [expr]", "- [expr]"]
NAMES = {"[expr] BETWEEN [expr] AND [expr]": "BETWEEN", "[expr] COLLATE (collation-name)": "COLLATE", "[expr] ESCAPE [escape-character-expr]": "ESCAPE",
         "[expr] ISNULL": "ISNULL", "[expr] NOTNULL": "NOTNULL", "[expr] NOT NULL": "NOT NULL", "NOT [expr]": "NOT", "~ [expr]": "~",
         "+ [expr]": "+ (unary)", "- [expr]": "- (unary)"}
operators = []
for cell in cells:
    rest = " ".join(cell.split())
    for m in MULTI:
        if m in rest:
            operators.append(NAMES.get(m, m))
            rest = rest.replace(m, " ")
    operators += [tok for tok in rest.split() if tok]
prefixable = re.search(r"Each keyword in \(([A-Z ]+)\) may be prefixed by NOT", re.sub(r"\s+", " ", html.unescape(re.sub(r"<[^>]+>", "", ex_raw))))
if not prefixable:
    raise SystemExit("lang_expr.html no longer says which keywords take NOT")
operators += [f"NOT {k}" for k in prefixable.group(1).split()]
ex = text_with_anchors("lang_expr.html")
for form, heading in [("EXISTS", "The EXISTS operator"), ("NOT EXISTS", "The EXISTS operator"), ("CASE", "The CASE expression"), ("CAST", "CAST expressions")]:
    if heading not in ex:
        raise SystemExit(f"lang_expr.html has no section '{heading}'")
    operators.append(form)
for op in dict.fromkeys(operators):
    add("operator", op, "lang_expr.html", "operators")

# Bind parameters, each form the expression page lists.
for form in ["?NNN", "?", ":AAAA", "@AAAA", "$AAAA"]:
    if form not in ex:
        raise SystemExit(f"lang_expr.html doesn't list the parameter form {form}")
    add("parameter", form, "lang_expr.html", "varparam")

# Expression forms with sections of their own: subqueries, correlated subqueries, row values,
# boolean expressions.
for name, page, phrase in [("scalar subquery", "lang_expr.html", "Subquery Expressions"), ("correlated subquery", "lang_expr.html", "Correlated Subqueries"),
                           ("row value", "rowvalue.html", "Row Values"), ("boolean expression", "lang_expr.html", "Boolean Expressions")]:
    if phrase not in text_with_anchors(page):
        raise SystemExit(f"{page} has no '{phrase}'")
    add("expression", name, page)

# 5. Types: the affinities, the built-in collations and the storage classes.
ty = text_with_anchors("datatype3.html")
for aff in ["TEXT", "NUMERIC", "INTEGER", "REAL", "BLOB"]:
    if f"{aff}" in ty:
        add("affinity", aff, "datatype3.html", "type_affinity")
for coll in ["BINARY", "NOCASE", "RTRIM"]:
    add("collation", coll, "datatype3.html", "collation")
for cls in ["NULL", "INTEGER", "REAL", "TEXT", "BLOB"]:
    add("storage_class", cls, "datatype3.html", "storage_classes_and_datatypes")

# The rules of the datatype page: how a column's declared type gives its affinity, which
# expressions carry affinity, how values of different classes compare and sort, and which collation
# a comparison or sort uses. Each entry quotes its rule; a rule whose words are gone stops the build.
RULES = [
    ("rule.affinity", "INT gives INTEGER", "affinity", 'If the declared type contains the string "INT" then it is assigned INTEGER affinity.'),
    ("rule.affinity", "CHAR, CLOB or TEXT gives TEXT", "affinity", 'If the declared type of the column contains any of the strings "CHAR", "CLOB", or "TEXT" then that column has TEXT affinity.'),
    ("rule.affinity", "BLOB or no type gives BLOB", "affinity", 'If the declared type for a column contains the string "BLOB" or if no type is specified then the column has affinity BLOB.'),
    ("rule.affinity", "REAL, FLOA or DOUB gives REAL", "affinity", 'If the declared type for a column contains any of the strings "REAL", "FLOA", or "DOUB" then the column has REAL affinity.'),
    ("rule.affinity", "otherwise NUMERIC", "affinity", "Otherwise, the affinity is NUMERIC."),
    ("rule.affinity", "the rules apply in order", "affinity", "Note that the order of the rules for determining column affinity is important."),
    ("rule.expression-affinity", "an IN list has no affinity", "affinity", "The right-hand operand of an IN or NOT IN operator has no affinity if the operand is a list"),
    ("rule.expression-affinity", "a column keeps its affinity", "affinity", "When an expression is a simple reference to a column of a real table (not a VIEW or subquery) then the expression has the same affinity as the table column."),
    ("rule.expression-affinity", "an operator removes affinity", "affinity", 'Any operators applied to column names, including the no-op unary "+" operator, convert the column name into an expression which always has no affinity.'),
    ("rule.expression-affinity", "CAST gives its type's affinity", "affinity", "has an affinity that is the same as a column with a declared type of"),
    ("rule.expression-affinity", "COLLATE keeps its operand's affinity", "affinity", "A COLLATE operator has the same affinity as its left-hand side operand."),
    ("rule.expression-affinity", "otherwise no affinity", "affinity", "Otherwise, an expression has no affinity."),
    ("rule.expression-affinity", "view and subquery columns", "affview", "the affinity for columns of a VIEW or subquery are determined by the expression affinity rules above"),
    ("rule.expression-affinity", "compound view columns", "affview", "the affinity of each column of the VIEW or subquery will be the affinity of the corresponding result column for one of the individual SELECT statements"),
    ("rule.comparison", "NULL is less than any value", "comparisons", "A value with storage class NULL is considered less than any other value"),
    ("rule.comparison", "numbers before text and blobs", "comparisons", "An INTEGER or REAL value is less than any TEXT or BLOB value."),
    ("rule.comparison", "numbers compare numerically", "comparisons", "When an INTEGER or REAL is compared to another INTEGER or REAL, a numerical comparison is performed."),
    ("rule.comparison", "text before blobs", "comparisons", "A TEXT value is less than a BLOB value."),
    ("rule.comparison", "text compares by collation", "comparisons", "When two TEXT values are compared an appropriate collating sequence is used to determine the result."),
    ("rule.comparison", "blobs compare by memcmp", "comparisons", "When two BLOB values are compared, the result is determined using memcmp()."),
    ("rule.comparison", "numeric affinity goes to the other operand", "compaff", "If one operand has INTEGER, REAL or NUMERIC affinity and the other operand has TEXT or BLOB or no affinity then NUMERIC affinity is applied to the other operand."),
    ("rule.comparison", "text affinity goes to an operand with none", "compaff", "If one operand has TEXT affinity and the other has no affinity, then TEXT affinity is applied to the other operand."),
    ("rule.comparison", "otherwise compared as is", "compaff", "Otherwise, no affinity is applied and both operands are compared as is."),
    ("rule.comparison", "BETWEEN is two comparisons", "compaff", 'The expression "a BETWEEN b AND c" is treated as two separate binary comparisons'),
    ("rule.comparison", "IN compares as equality", "compaff", 'Datatype conversions in comparisons of the form "x IN (SELECT y ...)" are handled as if the comparison were really "x=y".'),
    ("rule.sort", "ORDER BY orders by storage class", "comparisons", "values with storage class NULL come first, followed by INTEGER and REAL values interspersed in numeric order, followed by TEXT values in collating sequence order, and finally BLOB values in memcmp() order."),
    ("rule.sort", "GROUP BY keeps classes apart", "comparisons", "values with different storage classes are considered distinct, except for INTEGER and REAL values which are considered equal if they are numerically equal."),
    ("rule.sort", "compound SELECTs compare as is", "comparisons", "No affinity is applied to comparison operands for the implicit comparisons associated with UNION, INTERSECT, or EXCEPT"),
    ("rule.collation", "columns default to BINARY", "collation", "If no collating function is explicitly defined, then the collating function defaults to BINARY."),
    ("rule.collation", "an explicit COLLATE wins, left first", "collation", "then the explicit collating function is used for comparison, with precedence to the collating function of the left operand."),
    ("rule.collation", "then a column's collation, left first", "collation", "If either operand is a column, then the collating function of that column is used with precedence to the left operand."),
    ("rule.collation", "otherwise BINARY", "collation", "Otherwise, the BINARY collating function is used for comparison."),
    ("rule.collation", "the leftmost COLLATE anywhere wins", "collation", "the left most explicit collating function is used regardless of how deeply the COLLATE operators are nested"),
    ("rule.collation", "IN uses the left operand's collation", "collation", 'The collating sequence used for expressions of the form "x IN (y, z, ...)" is the collating sequence of x.'),
    ("rule.collation", "ORDER BY uses COLLATE, then the column's", "collation", "Otherwise, if the expression sorted by an ORDER BY clause is a column, then the collating sequence of the column is used to determine sort order."),
]
ty_plain = re.sub(r"\s+", " ", re.sub(r"\s+([,.])", r"\1", ty))
for kind, name, anchor, quote in RULES:
    if quote not in ty_plain and quote not in ty:
        raise SystemExit(f"datatype3.html no longer says: {quote}")
    add(kind, name, "datatype3.html", anchor, quote=quote)

# 6. Keywords, and the words the syntax diagrams use that aren't keywords (TRUE, FALSE, STRICT,
# STORED, ROWID, EXPRESSIONS, the exponent's E): SQLite reads them by their place in a statement.
keywords = re.findall(r"<li>([A-Z_]+)</li>", raw("lang_keywords.html"))
for k in keywords:
    add("keyword", k, "lang_keywords.html")
words = {}
for f_ in features:
    if f_["kind"] == "syntax":
        for w in f_["keywords"]:
            if w not in keywords:
                words.setdefault(w, []).append(f_["name"])
for w in sorted(words):
    add("word", w, f"syntax/{words[w][0]}.html", productions=sorted(words[w]))

counts = {}
for f in features:
    counts[f["kind"]] = counts.get(f["kind"], 0) + 1
with open(OUT, "w") as f:
    json.dump({"sqlite_doc": os.path.basename(os.path.abspath(DOC)), "counts": counts, "features": features}, f, indent=1)
print(json.dumps(counts, indent=1))
print(len(features), "entries")
