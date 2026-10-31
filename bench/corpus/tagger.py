#!/usr/bin/env python3
"""Each statement's features and shape (G2 step 2): the inventory entries (features.json) a statement
in SQLite's dialect exercises, read from its tokens, and the shape of the statement around them. A
query type is a shape with a set of features; the corpus counts distinct query types and how many
statements exercise each entry.

Features come from the tokens: function calls by name (a max or min of two or more arguments is the
scalar one; with OVER, a window call); operators by symbol and keyword (unary + and - apart from
binary ones; the AND of a BETWEEN apart from the logical one); keywords, context words, collations,
the affinity of a CAST's type, literals' storage classes, parameters, date modifiers in a date
function's arguments, PRAGMAs by name; syntax productions by the clauses that make them; scalar and
correlated subqueries (a subquery naming a table from outside it) and row values; and the affinity
rules a CREATE TABLE's declared types meet. The datatype page's comparison, sort and collation rules
can't be seen in the text: the written cases carry those entries by name.

usage: tagger.py <features.json> "<sql>"   prints the statement's features and shape
"""
import json
import re
import sqlite3
import sys

TOKEN = re.compile(r"""
    (?P<ws>\s+)
  | (?P<comment>--[^\n]*|/\*.*?(?:\*/|\Z))
  | (?P<blob>[xX]'[0-9a-fA-F]*')
  | (?P<string>'(?:[^']|'')*')
  | (?P<qident>"(?:[^"]|"")*"|\[[^\]]*\]|`[^`]*`)
  | (?P<number>0[xX][0-9a-fA-F_]+|(?:\d[\d_]*\.?[\d_]*|\.\d[\d_]*)(?:[eE][+-]?\d+)?)
  | (?P<param>\?\d*|[:@$][A-Za-z_][A-Za-z_0-9]*)
  | (?P<word>[A-Za-z_][A-Za-z_0-9$]*)
  | (?P<op>\|\||->>|->|<<|>>|<=|>=|==|!=|<>|[=<>+\-*/%&|~])
  | (?P<punct>[(),;.])
""", re.X | re.S)

AGGREGATES = {"avg", "count", "group_concat", "string_agg", "max", "min", "sum", "total", "median", "percentile", "percentile_cont", "percentile_disc",
              "json_group_array", "json_group_object", "jsonb_group_array", "jsonb_group_object"}
DATE_FUNCS = {"date", "time", "datetime", "julianday", "unixepoch", "strftime", "timediff"}
MODIFIER = [
    (re.compile(r"^[+-]?\s*\d+(\.\d+)?\s*days?$", re.I), "NNN days"), (re.compile(r"^[+-]?\s*\d+(\.\d+)?\s*hours?$", re.I), "NNN hours"),
    (re.compile(r"^[+-]?\s*\d+(\.\d+)?\s*minutes?$", re.I), "NNN minutes"), (re.compile(r"^[+-]?\s*\d+(\.\d+)?\s*seconds?$", re.I), "NNN seconds"),
    (re.compile(r"^[+-]?\s*\d+(\.\d+)?\s*months?$", re.I), "NNN months"), (re.compile(r"^[+-]?\s*\d+(\.\d+)?\s*years?$", re.I), "NNN years"),
    (re.compile(r"^start of month$", re.I), "start of month"), (re.compile(r"^start of year$", re.I), "start of year"),
    (re.compile(r"^start of day$", re.I), "start of day"), (re.compile(r"^weekday \d$", re.I), "weekday N"),
    (re.compile(r"^unixepoch$", re.I), "unixepoch"), (re.compile(r"^julianday$", re.I), "julianday"), (re.compile(r"^auto$", re.I), "auto"),
    (re.compile(r"^localtime$", re.I), "localtime"), (re.compile(r"^utc$", re.I), "utc"), (re.compile(r"^subsec$", re.I), "subsec"),
    (re.compile(r"^subsecond$", re.I), "subsecond"), (re.compile(r"^ceiling$", re.I), "ceiling"), (re.compile(r"^floor$", re.I), "floor"),
]
STATEMENTS = {  # first keyword(s) -> production
    "SELECT": "select-stmt", "VALUES": "select-stmt", "WITH": "select-stmt", "INSERT": "insert-stmt", "REPLACE": "insert-stmt",
    "UPDATE": "update-stmt", "DELETE": "delete-stmt", "ANALYZE": "analyze-stmt", "ATTACH": "attach-stmt", "DETACH": "detach-stmt",
    "BEGIN": "begin-stmt", "COMMIT": "commit-stmt", "END": "commit-stmt", "ROLLBACK": "rollback-stmt", "SAVEPOINT": "savepoint-stmt",
    "RELEASE": "release-stmt", "REINDEX": "reindex-stmt", "VACUUM": "vacuum-stmt", "PRAGMA": "pragma-stmt", "ALTER": "alter-table-stmt",
}
CREATE = {"TABLE": "create-table-stmt", "INDEX": "create-index-stmt", "VIEW": "create-view-stmt", "TRIGGER": "create-trigger-stmt", "VIRTUAL": "create-virtual-table-stmt"}
DROP = {"TABLE": "drop-table-stmt", "INDEX": "drop-index-stmt", "VIEW": "drop-view-stmt", "TRIGGER": "drop-trigger-stmt"}
BEFORE_UNARY = {"op", "start", "(", ",", "kw"}
VALUE_KINDS = {"number", "string", "blob", "qident", "ident", "param", ")"}
CLAUSE_KEYWORDS = {"SELECT", "WHERE", "AND", "OR", "NOT", "THEN", "WHEN", "ELSE", "ON", "HAVING", "BY", "SET", "RETURNING", "IN", "IS", "LIKE",
                   "GLOB", "BETWEEN", "CASE", "VALUES", "LIMIT", "OFFSET", "DO", "AS", "DISTINCT", "ALL", "MATCH", "REGEXP", "ESCAPE", "EXISTS", "RETURN"}


def affinity_rules(decl):
    """The column affinity rules a declared type meets, in order (datatype3.html 3.1)."""
    t = decl.upper()
    hits = []
    if "INT" in t:
        hits.append("INT gives INTEGER")
    if any(s in t for s in ("CHAR", "CLOB", "TEXT")):
        hits.append("CHAR, CLOB or TEXT gives TEXT")
    if "BLOB" in t or not t.strip():
        hits.append("BLOB or no type gives BLOB")
    if any(s in t for s in ("REAL", "FLOA", "DOUB")):
        hits.append("REAL, FLOA or DOUB gives REAL")
    if not hits:
        hits.append("otherwise NUMERIC")
    rules = hits[:1]
    if len(hits) > 1:
        rules.append("the rules apply in order")
    return rules


def cast_affinity(decl):
    first = affinity_rules(decl)[0]
    return {"INT gives INTEGER": "INTEGER", "CHAR, CLOB or TEXT gives TEXT": "TEXT", "BLOB or no type gives BLOB": "BLOB",
            "REAL, FLOA or DOUB gives REAL": "REAL", "otherwise NUMERIC": "NUMERIC"}[first]


class Tagger:
    def __init__(self, features_json):
        inv = json.load(open(features_json))["features"]
        self.ids = {f["id"] for f in inv}
        self.functions = {}
        for f in inv:
            if f["kind"].startswith("function."):
                self.functions.setdefault(f["name"], []).append(f["id"])
        self.keywords = {f["name"] for f in inv if f["kind"] == "keyword"}
        self.words = {f["name"] for f in inv if f["kind"] == "word"}
        self.pragmas = {f["name"] for f in inv if f["kind"] == "pragma"}

    def tokens(self, sql):
        out = []
        for m in TOKEN.finditer(sql):
            kind = m.lastgroup
            text = m.group()
            if kind == "ws":
                continue
            if kind == "word":
                up = text.upper()
                out.append(("kw" if up in self.keywords else "ident", text, up))
            elif kind == "punct" and text in "()":
                out.append((text, text, text))
            else:
                out.append((kind, text, text.upper()))
        return out

    def tag(self, sql):
        f = set()
        toks = self.tokens(sql)
        if not toks:
            return f, ()
        add = lambda i: f.add(i) if i in self.ids else None
        ups = [t[2] for t in toks]
        kinds = [t[0] for t in toks]
        n = len(toks)

        def nxt(i, k=1):
            return ups[i + k] if i + k < n else ""

        # statement kinds (EXPLAIN in front makes it an sql-stmt; several statements an sql-stmt-list)
        if ";" in ups and statement_count(sql) > 1:
            add("syntax:sql-stmt-list")
        start = 0
        if ups[0] == "EXPLAIN":
            add("syntax:sql-stmt")
            start = 3 if nxt(0) == "QUERY" else 1
        head = ups[start] if start < n else ""
        kind = head
        if head == "CREATE":
            j = start + 1
            while j < n and ups[j] in ("TEMP", "TEMPORARY", "UNIQUE"):
                j += 1
            kind = "CREATE " + (ups[j] if j < n else "")
            add("syntax:" + CREATE.get(ups[j] if j < n else "", "create-table-stmt"))
        elif head == "DROP":
            kind = "DROP " + nxt(start)
            add("syntax:" + DROP.get(nxt(start), "drop-table-stmt"))
        elif head in STATEMENTS:
            add("syntax:" + STATEMENTS[head])
        if any(t[0] == "comment" for t in toks):
            add("syntax:comment-syntax")

        depth = 0
        between_depths = []
        paren_stack = []          # for each open paren: what it opened
        scopes = [set()]          # table names and aliases declared at each subquery depth
        declared = set()          # every table name and alias in the statement
        pending = []              # each subquery's qualified names and its own tables
        subqueries = correlated = 0
        aggregate = window = False
        n_from = 0
        in_create_table = kind == "CREATE TABLE"
        col_types = []
        expect_type = False
        for i, (k, text, up) in enumerate(toks):
            prev_k = kinds[i - 1] if i else "start"
            prev_u = ups[i - 1] if i else ""
            # keywords and words
            if k == "kw":
                add(f"keyword:{up}")
            if up in self.words and k in ("kw", "ident"):
                add(f"word:{up}")
            if k == "number" and re.search(r"[eE]", text) and not text.lower().startswith("0x"):
                add("word:E")
            # literals
            if k == "number":
                add("syntax:numeric-literal")
                add("syntax:literal-value")
                add("storage_class:REAL" if re.search(r"[.eE]", text) and not text.lower().startswith("0x") else "storage_class:INTEGER")
            elif k == "string":
                add("syntax:literal-value")
                add("storage_class:TEXT")
            elif k == "blob":
                add("syntax:literal-value")
                add("storage_class:BLOB")
            elif up == "NULL" and k == "kw" and prev_u not in ("NOT", "IS"):
                add("syntax:literal-value")
                add("storage_class:NULL")
            elif up in ("CURRENT_DATE", "CURRENT_TIME", "CURRENT_TIMESTAMP"):
                add("syntax:literal-value")
            if k == "param":
                add("parameter:" + ("?NNN" if re.fullmatch(r"\?\d+", text) else "?" if text == "?" else text[0] + "AAAA"))
            # parentheses, subqueries and row values
            if k == "(":
                opener = "group"
                if prev_k in ("ident", "kw") and prev_u not in CLAUSE_KEYWORDS and prev_u not in ("FROM", "JOIN", "USING", "OVER", "FILTER", "AS", "WINDOW") or (i >= 2 and ups[i - 2] == "."):
                    opener = "call"
                if nxt(i) in ("SELECT", "WITH", "VALUES"):
                    opener = "subquery"
                    subqueries += 1
                    if prev_u not in ("FROM", "JOIN", ",", "AS") or prev_u == ",":
                        add("expression:scalar subquery")
                    if prev_u in ("FROM", "JOIN") or (prev_u == "," and n_from):
                        add("syntax:table-or-subquery")
                    scopes.append(set())
                paren_stack.append((opener, i))
                depth += 1
            elif k == ")":
                if paren_stack:
                    opener, at = paren_stack.pop()
                    if opener == "subquery":
                        inner = scopes.pop()
                        quals = {ups[j - 1].lower() for j in range(at + 1, i) if ups[j] == "." and j >= 1 and kinds[j - 1] in ("ident", "qident")}
                        pending.append((quals, inner))
                    elif opener == "group":
                        commas = sum(1 for j in range(at + 1, i) if ups[j] == "," and depth_at(toks, at, j) == 1)
                        before = ups[at - 1] if at else ""
                        after = nxt(i)
                        if commas and before not in ("IN", "VALUES", "USING", "TABLE", "INTO", "ON", ",") and after in ("=", "==", "<", ">", "<=", ">=", "<>", "!=", "IS", "IN", "BETWEEN", "NOT"):
                            add("expression:row value")
                depth -= 1
                if between_depths and between_depths[-1] > depth:
                    between_depths.pop()
            # function calls
            if k in ("ident", "kw") and nxt(i) == "(":
                name = text.lower()
                if name in self.functions and not (prev_u in ("TABLE", "INTO", "INDEX", "VIEW", "TRIGGER", "ON", "REFERENCES") and name not in ("json_each", "json_tree", "jsonb_each", "jsonb_tree")):
                    nargs = count_args(toks, i + 1)
                    over = following_over(toks, i + 1)
                    ids = self.functions[name]
                    pick = None
                    if name in ("max", "min"):
                        pick = f"function.core:{name}" if nargs >= 2 else f"function.aggregate:{name}"
                    elif len(ids) == 1:
                        pick = ids[0]
                    else:
                        pick = next((x for x in ids if x.startswith("function.window")), ids[0]) if over else ids[0]
                    add(pick)
                    add("syntax:function-arguments" if nargs else "syntax:simple-function-invocation")
                    if over:
                        window = True
                        add("syntax:window-function-invocation")
                    elif name in AGGREGATES and not (name in ("max", "min") and nargs >= 2):
                        aggregate = True
                        add("syntax:aggregate-function-invocation")
                    else:
                        add("syntax:simple-function-invocation")
                    if name in DATE_FUNCS:
                        for s in call_strings(toks, i + 1):
                            for rx, mod in MODIFIER:
                                if rx.match(s.strip()):
                                    add(f"datetime.modifier:{mod}")
            # operators
            if k == "op":
                if text in ("+", "-") and (prev_k in BEFORE_UNARY or prev_u in CLAUSE_KEYWORDS) and prev_k not in VALUE_KINDS:
                    add(f"operator:{text} (unary)")
                elif text == "*" and (prev_k not in VALUE_KINDS or nxt(i) in (")", ",", "FROM")):
                    pass  # SELECT *, t.*, count(*)
                elif text == "~":
                    add("operator:~")
                else:
                    add(f"operator:{text}")
            if k == "kw":
                if up == "IS":
                    if nxt(i) == "NOT" and nxt(i, 2) == "DISTINCT":
                        add("operator:IS NOT DISTINCT FROM")
                    elif nxt(i) == "DISTINCT":
                        add("operator:IS DISTINCT FROM")
                    elif nxt(i) == "NOT":
                        add("operator:IS NOT")
                    else:
                        add("operator:IS")
                elif up == "NOT":
                    if nxt(i) in ("IN", "LIKE", "GLOB", "MATCH", "REGEXP", "BETWEEN", "EXISTS"):
                        add(f"operator:NOT {nxt(i)}")
                    elif nxt(i) == "NULL" and prev_u != "IS" and not in_create_table:
                        add("operator:NOT NULL")
                    elif prev_u != "IS" and nxt(i) != "NULL" and not (in_create_table and nxt(i) in ("NULL",)) and prev_u not in ("IF",) and nxt(i) not in ("EXISTS", "INDEXED", "MATERIALIZED", "DEFERRABLE"):
                        add("operator:NOT")
                elif up in ("IN", "LIKE", "GLOB", "MATCH", "REGEXP", "ESCAPE", "ISNULL", "NOTNULL", "CASE", "CAST", "COLLATE", "EXISTS") and prev_u != "NOT":
                    if not (up == "IN" and in_create_table) and not (up == "MATCH" and prev_u == "MATCH"):
                        add(f"operator:{up}")
                elif up == "BETWEEN" and prev_u != "NOT":
                    add("operator:BETWEEN")
                    between_depths.append(depth + 1)
                elif up == "BETWEEN":
                    between_depths.append(depth + 1)
                elif up == "AND":
                    if between_depths and between_depths[-1] == depth + 1:
                        between_depths.pop()
                    elif not in_create_table or depth:
                        add("operator:AND")
                elif up == "OR" and prev_u not in ("INSERT", "UPDATE", "REPLACE"):
                    add("operator:OR")
                elif up == "OR":
                    add("syntax:conflict-clause")
                if up == "COLLATE" and nxt(i):
                    add(f"collation:{nxt(i)}")
                if up == "CAST" and nxt(i) == "(":
                    end = close_of(toks, i + 1)
                    d, as_at = 0, None
                    for j in range(i + 1, end):
                        d += 1 if kinds[j] == "(" else -1 if kinds[j] == ")" else 0
                        if d == 1 and ups[j] == "AS":
                            as_at = j
                    if as_at is not None:
                        words = []
                        for j in range(as_at + 1, end):
                            if kinds[j] == "(":
                                break
                            words.append(toks[j][1])
                        add(f"affinity:{cast_affinity(' '.join(words))}")
                # clauses
                if up == "FROM":
                    add("syntax:table-or-subquery")
                    n_from = max(n_from, 1)
                elif up == "JOIN":
                    add("syntax:join-clause")
                    add("syntax:join-operator")
                    n_from += 1
                elif up in ("NATURAL", "CROSS", "LEFT", "RIGHT", "FULL", "INNER", "OUTER"):
                    add("syntax:join-operator")
                elif up in ("ON", "USING") and n_from > 1 and not in_create_table:
                    add("syntax:join-constraint")
                elif up in ("UNION", "INTERSECT", "EXCEPT"):
                    add("syntax:compound-select-stmt")
                    add("syntax:compound-operator")
                elif up == "RECURSIVE":
                    add("syntax:recursive-cte")
                elif up == "OVER":
                    add("syntax:over-clause")
                elif up == "WINDOW" and nxt(i, 2) == "AS":
                    add("syntax:window-defn")
                elif up in ("ROWS", "RANGE", "GROUPS") and (nxt(i) in ("BETWEEN", "UNBOUNDED", "CURRENT") or kinds[i + 1 if i + 1 < n else i] == "number"):
                    add("syntax:frame-spec")
                elif up == "FILTER" and nxt(i) == "(":
                    add("syntax:filter-clause")
                elif up == "RETURNING":
                    add("syntax:returning-clause")
                elif up == "CONFLICT" and prev_u == "ON":
                    add("syntax:upsert-clause" if kind in ("INSERT", "REPLACE") else "syntax:conflict-clause")
                elif up == "INDEXED" or (up == "NOT" and nxt(i) == "INDEXED"):
                    add("syntax:qualified-table-name")
                elif up == "RAISE":
                    add("syntax:raise-function")
                elif up in ("WITHOUT", "STRICT") and in_create_table and depth == 0:
                    add("syntax:table-options")
                elif up == "REFERENCES":
                    add("syntax:foreign-key-clause")
                elif up == "WITH" and i == start:
                    add("syntax:with-clause")
                    add("syntax:common-table-expression")
                    add("syntax:factored-select-stmt")
                elif up in ("ORDER",) and nxt(i) == "BY":
                    add("syntax:ordering-term")
            if up == "SET" and nxt(i) == "(":
                add("syntax:column-name-list")
            # tables and aliases in scope (for correlation)
            if prev_u in ("FROM", "JOIN", ",", "UPDATE", "INTO") and k in ("ident", "qident") and (n_from or prev_u in ("UPDATE", "INTO")):
                names = [text.strip('"[]`').lower()]
                if nxt(i) in ("AS",) and i + 2 < n:
                    names.append(toks[i + 2][1].strip('"[]`').lower())
                elif i + 1 < n and kinds[i + 1] == "ident":
                    names.append(toks[i + 1][1].strip('"[]`').lower())
                scopes[-1].update(names)
                declared.update(names)
            # CREATE TABLE: column definitions, their types, constraints
            if in_create_table and depth == 1 and k in ("ident", "qident", "kw") and prev_u in ("(", ","):
                if up not in ("PRIMARY", "UNIQUE", "CHECK", "FOREIGN", "CONSTRAINT"):
                    add("syntax:column-def")
                    expect_type = True
                    col_types.append("")
                    continue
                add("syntax:table-constraint")
            if in_create_table and expect_type:
                if k in ("ident", "kw") and up not in ("PRIMARY", "NOT", "NULL", "UNIQUE", "CHECK", "DEFAULT", "COLLATE", "REFERENCES", "GENERATED", "AS", "CONSTRAINT"):
                    col_types[-1] = (col_types[-1] + " " + text).strip()
                    add("syntax:type-name")
                    if nxt(i) == "(":
                        add("syntax:signed-number")
                    continue
                expect_type = False
            if in_create_table and depth == 1 and up in ("PRIMARY", "NOT", "UNIQUE", "CHECK", "DEFAULT", "COLLATE", "REFERENCES", "GENERATED") and prev_u not in ("(", ","):
                add("syntax:column-constraint")
            if in_create_table and up == "AS" and nxt(i) == "(" and depth == 1:
                add("syntax:column-constraint")
        # a subquery naming a table it doesn't declare, one the statement declares elsewhere, is correlated
        for quals, inner in pending:
            if any(q not in inner and q in declared for q in quals):
                correlated += 1
                add("expression:correlated subquery")
        if in_create_table:
            for t in col_types:
                for r in affinity_rules(t):
                    add(f"rule.affinity:{r}")
        if kind in ("SELECT", "VALUES", "WITH"):
            add("syntax:select-core")
            if "SELECT" in ups:
                add("syntax:result-column")
            if "syntax:compound-select-stmt" not in f and "syntax:with-clause" not in f:
                add("syntax:simple-select-stmt")
        if kind in ("UPDATE", "DELETE") and ("LIMIT" in ups or "ORDER" in ups):
            add(f"syntax:{'update' if kind == 'UPDATE' else 'delete'}-stmt-limited")
        if kind == "CREATE INDEX":
            add("syntax:indexed-column")
        if kind == "PRAGMA":
            j = start + 1
            name = (ups[j + 2] if nxt(start, 2) == "." else ups[j] if j < n else "").strip('"[]`').lower()
            if name in self.pragmas:
                add(f"pragma:{name}")
            if "=" in ups or ("(" in ups):
                add("syntax:pragma-value")
        if any(x.startswith("operator:") or x.startswith("function.") for x in f) or "syntax:literal-value" in f:
            add("syntax:expr")
        if bare_condition(toks):
            add("expression:boolean expression")
        shape = (kind, min(n_from, 3), min(subqueries, 2), aggregate, "GROUP" in ups, "HAVING" in ups, "syntax:ordering-term" in f,
                 "LIMIT" in ups, "DISTINCT" in ups, "syntax:compound-select-stmt" in f, window, "syntax:with-clause" in f)
        return f, shape


def statement_count(sql):
    """Statements in the text, where SQLite's own parser says each ends (a trigger's body is one
    statement's), not counting what's only whitespace or comments."""
    count, cur = 0, ""
    for piece in sql.split(";"):
        cur = piece if not cur else cur + ";" + piece
        if sqlite3.complete_statement(cur + ";"):
            if any(m.lastgroup not in ("ws", "comment") for m in TOKEN.finditer(cur)):
                count += 1
            cur = ""
    return count + (1 if cur.strip() and any(m.lastgroup not in ("ws", "comment") for m in TOKEN.finditer(cur)) else 0)


def depth_at(toks, at, j):
    d = 0
    for t in toks[at:j]:
        if t[0] == "(":
            d += 1
        elif t[0] == ")":
            d -= 1
    return d


def count_args(toks, open_i):
    d, args, seen = 0, 0, False
    for t in toks[open_i:]:
        if t[0] == "(":
            d += 1
            continue
        if t[0] == ")":
            d -= 1
            if d == 0:
                return args + (1 if seen else 0)
            continue
        if d == 1:
            if t[1] == ",":
                args += 1
            elif t[2] != "DISTINCT":
                seen = True
    return args


def close_of(toks, open_i):
    d = 0
    for j in range(open_i, len(toks)):
        if toks[j][0] == "(":
            d += 1
        elif toks[j][0] == ")":
            d -= 1
            if d == 0:
                return j
    return len(toks) - 1


def following_over(toks, open_i):
    j = close_of(toks, open_i) + 1
    if j < len(toks) and toks[j][2] == "FILTER":
        j = close_of(toks, j + 1) + 1
    return j < len(toks) and toks[j][2] == "OVER"


def call_strings(toks, open_i):
    j = close_of(toks, open_i)
    return [t[1][1:-1].replace("''", "'") for t in toks[open_i:j] if t[0] == "string"]


def bare_condition(toks):
    """A condition that's a bare value (WHERE x, x AND y with x not compared): SQLite reads its
    truth from its number."""
    ups = [t[2] for t in toks]
    between = 0
    for i, u in enumerate(ups):
        if u == "BETWEEN":
            between += 1
            continue
        if u == "AND" and between:
            between -= 1
            continue
        if u in ("WHERE", "HAVING", "WHEN", "AND", "OR", "NOT") and i + 2 <= len(ups):
            a = toks[i + 1] if i + 1 < len(toks) else None
            b = ups[i + 2] if i + 2 < len(ups) else ""
            if a and a[0] in ("ident", "number", "string") and b in ("", ";", ")", "AND", "OR", "THEN", "ORDER", "GROUP", "LIMIT", "HAVING"):
                return True
    return False


if __name__ == "__main__":
    t = Tagger(sys.argv[1])
    feats, shape = t.tag(sys.argv[2])
    print(sorted(feats))
    print(shape)
