#!/usr/bin/env python3
"""Each corpus record translated once per dialect (G2 step 4), at corpus build time, by sqlglot (the
translator sources.json pins), so the differential harness (crates/corpus, amphora_corpus::dialect)
runs every engine on SQL it speaks and never translates as it runs.

For every `.test` file under the pool (symbolic links followed: the pool links sqllogictest's
checkout), each record's SQL, in SQLite's dialect as the corpus holds it, is written in each dialect
to <pool>/translations/<dialect>/<file>.jsonl, a JSON line a record in file order:
  {"line": n, "sql": "..."}              its text in the dialect
  {"line": n, "untranslatable": "..."}   sqlglot can't say it there, or the dialect has nothing that
                                         means the same ("not expressible in <dialect>: ..."), and why
The harness classifies an untranslatable record as such, never as a disagreement. Engines that speak
SQLite's dialect (SQLite, Amphora, Turso) take the records as written and get no translation; the
postgres dialect is OrioleDB's and CedarDB's too.

usage: dialects.py <pool dir> [<dialect> ...]   (default: every dialect below)
"""
import json
import logging
import os
import re
import sys

import slt

DIALECTS = ("duckdb", "postgres", "mysql", "clickhouse")

logging.getLogger("sqlglot").setLevel(logging.ERROR)  # its "falling back to a Command" notes


class NotExpressible(Exception):
    """What a record says has no exact spelling in a dialect: the record is untranslatable there,
    with this as the reason the oracle reports, never dropped and never said approximately."""


# SQLite's reading, where sqlglot's SQLite reader refuses what SQLite takes (G2 step 4). Each form is
# read as SQLite's documentation says it means, so what is written in each dialect means the same.

# Words that end a column's type name: a column constraint starts with one of them (SQLite's
# "CREATE TABLE", column-constraint syntax, https://www.sqlite.org/lang_createtable.html).
# Words that start a table constraint (lang_createtable.html, table-constraint syntax).
_TABLE_CONSTRAINT_WORDS = {"CONSTRAINT", "PRIMARY", "UNIQUE", "CHECK", "FOREIGN"}
_CONSTRAINT_WORDS = {"CONSTRAINT", "PRIMARY", "NOT", "NULL", "UNIQUE", "CHECK", "DEFAULT", "COLLATE", "REFERENCES", "GENERATED", "AS", "ON"}


def affinity(name):
    """The affinity SQLite gives a column declared with type `name`, by "Datatypes In SQLite"
    3.1 "Determination Of Column Affinity" (https://www.sqlite.org/datatype3.html#determination_of_column_affinity),
    its rules in their order: "INT" gives INTEGER; "CHAR", "CLOB" or "TEXT" gives TEXT; "BLOB" or no
    type gives BLOB; "REAL", "FLOA" or "DOUB" gives REAL; anything else NUMERIC. Its own example:
    "FLOATING POINT" is INTEGER, "since "INT" occurs at the end of "POINT"."."""
    n = name.upper()
    if "INT" in n:
        return "INTEGER"
    if any(w in n for w in ("CHAR", "CLOB", "TEXT")):
        return "TEXT"
    if "BLOB" in n or not n.strip():
        return "BLOB"
    if any(w in n for w in ("REAL", "FLOA", "DOUB")):
        return "REAL"
    return "NUMERIC"


def _iif(args):
    """iif(B1,V1,...,Bn,Vn[,E]) and its alias if(): "a shorthand for "CASE WHEN B1 THEN V1 ... ELSE E
    END"", NULL when no B is true and E is left out (SQLite's "Built-In Scalar SQL Functions", iif,
    https://www.sqlite.org/lang_corefunc.html#iif)."""
    from sqlglot import exp

    pairs, default = (args, None) if len(args) % 2 == 0 else (args[:-1], args[-1])
    return exp.Case(ifs=[exp.If(this=pairs[i], true=pairs[i + 1]) for i in range(0, len(pairs), 2)], default=default)


def _reader():
    """sqlglot's SQLite dialect with the forms it refuses read as SQLite reads them."""
    from sqlglot import exp
    from sqlglot.dialects.sqlite import SQLite
    from sqlglot.tokens import TokenType

    class Parser(SQLite.Parser):
        FUNCTIONS = {**SQLite.Parser.FUNCTIONS, "IIF": _iif, "IF": _iif}
        # char(X1,...,XN): "a string composed of characters having the unicode code point values of
        # integers X1 through XN" (lang_corefunc.html#char): of none, the empty string.
        FUNCTION_PARSERS = {**SQLite.Parser.FUNCTION_PARSERS, "CHAR": lambda self: exp.Literal.string("") if self._curr and self._curr.token_type == TokenType.R_PAREN else self._parse_char()}

        def _parse_if(self):
            # if(), iif()'s alias in SQLite, rather than sqlglot's three-argument IF.
            if not self._match(TokenType.L_PAREN):
                return super()._parse_if()
            args = self._parse_csv(self._parse_assignment)
            self._match_r_paren()
            return _iif(args)

        def validate_expression(self, expression, args=None):
            # iif()'s CASE takes any number of arguments, not CASE's own three.
            return super().validate_expression(expression, None if isinstance(expression, exp.Case) else args)

        def _parse_id_var(self, *args, **kwargs):
            # A window's frame type GROUPS where a base window's name may stand (SQLite's "Window
            # Functions", 2.2 "Frame Specifications": "The frame type ... ROWS, GROUPS, or RANGE",
            # https://www.sqlite.org/windowfunctions.html#frame_specifications): sqlglot otherwise
            # takes it for the name when no PARTITION BY or ORDER BY comes first.
            if kwargs.get("tokens") is self.WINDOW_ALIAS_TOKENS and self._curr and self._curr.text.upper() == "GROUPS":
                return None
            return super()._parse_id_var(*args, **kwargs)

        def _parse_types(self, check_func=False, schema=False, allow_identifiers=True, with_collation=False):
            # A column's type name of several words ("FLOATING POINT", "UNSIGNED BIG INT"), or ANY:
            # SQLite's type-name is one or more names with an optional size (lang_createtable.html),
            # and what SQLite takes from it is its affinity alone (datatype3.html, 3.1: "the
            # declared type of the column determines the affinity of the column"), so the column is
            # declared with its affinity's own name. ANY is the affinity's name too, NUMERIC by rule
            # 5, except in a STRICT table (typed() gives it there).
            if schema and allow_identifiers:
                words, i = [], self._index
                while i < len(self._tokens):
                    t = self._tokens[i]
                    if t.text.upper() in _CONSTRAINT_WORDS or not (t.token_type in (TokenType.VAR, TokenType.ANY) or t.token_type in self.TYPE_TOKENS):
                        break
                    words.append(t.text)
                    i += 1
                if len(words) > 1 or (words and words[0].upper() == "ANY"):
                    self._advance(len(words))
                    if self._match(TokenType.L_PAREN):  # its size, which SQLite ignores (datatype3.html, 3.1)
                        while self._curr and not self._match(TokenType.R_PAREN):
                            self._advance()
                    kind = exp.DataType.build(affinity(" ".join(words)))
                    kind.meta["sqlite_any"] = words[0].upper() == "ANY" and len(words) == 1
                    return kind
            return super()._parse_types(check_func=check_func, schema=schema, allow_identifiers=allow_identifiers, with_collation=with_collation)

        def _parse_primary_key_part(self):
            # A table PRIMARY KEY's indexed-column: "expr [COLLATE name] [ASC|DESC]" (SQLite's "CREATE
            # TABLE", table-constraint and indexed-column syntax, https://www.sqlite.org/lang_createtable.html),
            # which SQLite's grammar also takes with AUTOINCREMENT after it. ASC and DESC order the key's
            # index alone: "CREATE TABLE t(x INTEGER, y, z, PRIMARY KEY(x ASC))" and "PRIMARY KEY(x DESC)"
            # make x the same rowid alias as "x INTEGER PRIMARY KEY ASC" (lang_createtable.html, "ROWIDs
            # and the INTEGER PRIMARY KEY"), so they are left out. COLLATE and AUTOINCREMENT are kept
            # on the column for keyed() to say.
            part = super()._parse_primary_key_part()
            if part is None:
                return part
            if self._match(TokenType.COLLATE):
                name = self._curr.text if self._curr else ""
                self._advance()
                part.meta["sqlite_collate"] = name.strip('"`[]').upper()
            self._match_set((TokenType.ASC, TokenType.DESC))
            if self._curr and self._curr.text.upper() == "AUTOINCREMENT":
                self._advance()
                part.meta["sqlite_autoincrement"] = True
            return part

        def _parse_schema(self, this=None):
            # sqlglot's own, with SQLite's table constraints taken without a comma between them:
            # SQLite's grammar reads "conslist ::= conslist tconscomma tcons" with "tconscomma ::= ."
            # beside "tconscomma ::= COMMA." (src/parse.y, https://sqlite.org/src/file?name=src/parse.y),
            # so "CHECK (a > 0) CHECK (a < 9)" is two constraints, as with the comma.
            index = self._index
            if not self._match(TokenType.L_PAREN):
                return this
            if self._match_set(self.SELECT_START_TOKENS):
                self._retreat(index)
                return this
            args = []
            while True:
                args.extend(self._parse_csv(lambda: self._parse_constraint() or self._parse_field_def()))
                if not (args and self._curr and self._curr.text.upper() in _TABLE_CONSTRAINT_WORDS):
                    break
            self._match_r_paren()
            return self.expression(exp.Schema(this=this, expressions=args))

        def _parse_statement(self):
            # VALUES after a WITH clause's common table expressions: SQLite's "SELECT" (https://www.sqlite.org/lang_select.html,
            # 2.6 "The VALUES clause"): "The phrase "VALUES(expr-list)" means the same thing as "SELECT
            # expr-list". The phrase "VALUES(expr-list-1),...,(expr-list-N)" means the same thing as
            # "SELECT expr-list-1 UNION ALL ... UNION ALL SELECT expr-list-N"", its columns named
            # column1, column2, ... (the same section), which sqlglot refuses after WITH ("values does
            # not support CTE"): said as those SELECTs.
            if self._curr and self._curr.token_type == TokenType.VALUES and self._prev and self._prev.token_type == TokenType.R_PAREN:
                self._advance()
                rows = self._parse_csv(self._parse_value)
                selects = [exp.select(*[(e.as_(f"column{i + 1}") if n == 0 else e) for i, e in enumerate(row.expressions)]) for n, row in enumerate(rows)]
                said = selects[0]
                for more in selects[1:]:
                    said = exp.union(said, more, distinct=False)
                return said
            # SAVEPOINT name, RELEASE [SAVEPOINT] name (https://www.sqlite.org/lang_savepoint.html)
            # and REINDEX [name] (https://www.sqlite.org/lang_reindex.html), which sqlglot reads as a
            # column and its alias: each as a command, RELEASE in its full spelling, which
            # PostgreSQL's and MySQL's is too ("RELEASE [ SAVEPOINT ] savepoint_name"; "RELEASE
            # SAVEPOINT identifier"). An engine without it refuses it, and the oracle classifies that.
            word = self._curr.text.upper() if self._curr and self._curr.token_type == TokenType.VAR else None
            if word not in ("SAVEPOINT", "RELEASE", "REINDEX"):
                return super()._parse_statement()
            self._advance()
            if word == "RELEASE":
                self._match_text_seq("SAVEPOINT")
            first = self._curr
            last = None
            while self._curr and self._curr.token_type != TokenType.SEMICOLON:
                last = self._curr
                self._advance()
            rest = self._find_sql(first, last) if last is not None else ""
            return exp.Command(this=word, expression=exp.Literal.string(("SAVEPOINT " if word == "RELEASE" else "") + rest))

    class Reader(SQLite):
        pass

    Reader.parser_class = Parser
    return Reader


_READER = None


def reader():
    global _READER
    if _READER is None:
        _READER = _reader()
    return _READER


def comment_closed(sql):
    """`sql` with a block comment left open at its end closed: SQLite's "SQL Comment Syntax"
    (https://www.sqlite.org/lang_comment.html): "C-style comments begin with "/*" and extend up to and
    including the next "*/" character pair or until the end of input, whichever comes first." Quoted
    text ('', "", ``, []) and line comments are read past as SQLite's tokenizer reads them."""
    i, n = 0, len(sql)
    while i < n:
        c = sql[i]
        if c in "'\"`[":
            close = "]" if c == "[" else c
            j = sql.find(close, i + 1)
            if j < 0:
                return sql  # an unterminated string: SQLite refuses it, and so does sqlglot
            i = j + 1
        elif sql.startswith("--", i):
            j = sql.find("\n", i)
            i = n if j < 0 else j + 1
        elif sql.startswith("/*", i):
            j = sql.find("*/", i + 2)
            if j < 0:
                return sql + "*/"
            i = j + 2
        else:
            i += 1
    return sql


# SQLite's conflict resolutions other than ABORT (https://www.sqlite.org/lang_conflict.html):
# ROLLBACK aborts the statement and rolls back the transaction; FAIL aborts it but keeps "prior
# changes of the SQL statement that failed"; IGNORE "skips the one row that contains the constraint
# violation and continues"; REPLACE deletes "pre-existing rows that are causing the constraint
# violation" (and, on NOT NULL, writes the column's default). On UPDATE (UPDATE OR <resolution>) and
# on a constraint (ON CONFLICT <resolution>) no engine the corpus compares has them: PostgreSQL's ON
# CONFLICT is INSERT's, on unique and exclusion constraints only; MySQL's UPDATE IGNORE writes a
# NOT NULL column's implicit default where SQLite skips the row; DuckDB and ClickHouse have none.
_RESOLUTIONS = ("ROLLBACK", "FAIL", "IGNORE", "REPLACE")


def conflict_clause(sql):
    """The first `UPDATE OR <resolution>` or `ON CONFLICT <resolution>` (other than ABORT) in `sql`,
    by SQLite's tokens, or None."""
    from sqlglot.dialects.sqlite import SQLite
    from sqlglot.tokens import TokenType

    try:
        tokens = SQLite().tokenize(sql)
    except Exception:  # sqlglot can't tokenize it: translate() says why
        return None
    quoted = (TokenType.STRING, TokenType.IDENTIFIER, TokenType.HEX_STRING, TokenType.BIT_STRING, TokenType.NATIONAL_STRING)
    words = [None if t.token_type in quoted else t.text.upper() for t in tokens] + [None, None]
    for i in range(len(tokens)):
        if words[i] == "UPDATE" and words[i + 1] == "OR" and words[i + 2] in _RESOLUTIONS:
            return f"UPDATE OR {words[i + 2]}"
        if words[i] == "ON" and words[i + 1] == "CONFLICT" and words[i + 2] in _RESOLUTIONS:
            return f"ON CONFLICT {words[i + 2]}"
    return None


def upserts(sql):
    """How many upsert clauses (ON CONFLICT followed by a conflict target or DO) the most of any one
    statement in `sql` has, by SQLite's tokens."""
    from sqlglot.dialects.sqlite import SQLite
    from sqlglot.tokens import TokenType

    try:
        tokens = SQLite().tokenize(sql)
    except Exception:  # sqlglot can't tokenize it: translate() says why
        return 0
    most, n = 0, 0
    for i, t in enumerate(tokens):
        if t.token_type == TokenType.SEMICOLON:
            most, n = max(most, n), 0
        elif t.token_type != TokenType.STRING and t.text.upper() == "ON" and i + 2 < len(tokens) and tokens[i + 1].text.upper() == "CONFLICT" and (tokens[i + 2].token_type == TokenType.L_PAREN or tokens[i + 2].text.upper() == "DO"):
            n += 1
    return max(most, n)


def _offset(value, side):
    """A frame bound as a signed row offset (None: not a literal), PRECEDING negative."""
    from sqlglot import exp

    if value == "UNBOUNDED":
        return float("-inf") if side == "PRECEDING" else float("inf")
    if value == "CURRENT ROW":
        return 0
    if isinstance(value, exp.Literal) and value.is_int:
        return -int(value.this) if side == "PRECEDING" else int(value.this)
    return None


def _around(window, lo, hi):
    """min, max, first_value or last_value over a ROWS frame [lo, hi] holding the current row, with
    EXCLUDE CURRENT ROW: the frame without the current row is the rows before it in the frame (ROWS
    lo .. 1 PRECEDING) and the rows after it (ROWS 1 FOLLOWING .. hi), as SQLite's 2.2.4 says
    ("EXCLUDE CURRENT ROW: the current row is excluded from the frame"). Over an empty frame each is
    NULL (windowfunctions.html 2.2, and an aggregate of no rows, lang_aggfunc.html: "max() returns
    NULL if and only if there are no non-NULL values in the group"). Exact, each part by the same
    function:
    - min (max) is the smaller (larger) of the two parts' min (max), NULLs skipped as min skips them;
    - first_value is the first row's: the part before when it has rows, else the part after;
      last_value the other way round."""
    from sqlglot import exp

    func = window.this

    def part(start, start_side, end, end_side, fn=None):
        w = window.copy()
        if fn is not None:
            w.set("this", fn)
        spec = w.args["spec"]
        spec.set("exclude", None)
        spec.set("start", start)
        spec.set("start_side", start_side)
        spec.set("end", end)
        spec.set("end_side", end_side)
        return w

    spec = window.args["spec"]
    before = part(spec.args.get("start"), spec.args.get("start_side"), exp.Literal.number(1), "PRECEDING") if lo < 0 else None
    after = part(exp.Literal.number(1), "FOLLOWING", spec.args.get("end"), spec.args.get("end_side")) if hi > 0 else None
    if before is None and after is None:
        return exp.Null()
    if before is None or after is None:
        return before or after
    if isinstance(func, (exp.Min, exp.Max)):
        pick = exp.LTE if isinstance(func, exp.Min) else exp.GTE
        return exp.Case(ifs=[exp.If(this=exp.Is(this=before.copy(), expression=exp.Null()), true=after.copy()), exp.If(this=exp.Is(this=after.copy(), expression=exp.Null()), true=before.copy()), exp.If(this=pick(this=before.copy(), expression=after.copy()), true=before.copy())], default=after.copy())
    rows = exp.Count(this=exp.Star())
    first, second = (before, after) if isinstance(func, exp.FirstValue) else (after, before)
    counted = first.copy()
    counted.set("this", rows)
    return exp.Case(ifs=[exp.If(this=exp.GT(this=counted, expression=exp.Literal.number(0)), true=first.copy())], default=second.copy())


def excluded(window, dialect):
    """`window`, whose frame has EXCLUDE CURRENT ROW, GROUP or TIES (SQLite's "Window Functions",
    2.2.4 "The EXCLUDE clause", https://www.sqlite.org/windowfunctions.html#the_exclude_clause), said
    without EXCLUDE where that means the same; NotExpressible where nothing does. MySQL and ClickHouse
    have no EXCLUDE (MySQL 8.4 Reference Manual, 14.20.3 "Window Function Frame Specification", and
    ClickHouse's "Window Functions", whose frames take no exclusion). Exact equivalents:
    - EXCLUDE CURRENT ROW on a ROWS frame that doesn't hold the current row (both bounds before it,
      or both after) excludes nothing: the clause is left out, for any function;
    - count(x) or count(*) over a frame that holds the current row: EXCLUDE CURRENT ROW takes away
      the current row's count (1 for count(*); 1 for count(x) when x isn't NULL); EXCLUDE GROUP takes
      away the current row's peers' count, which is count over the partition and ORDER BY terms
      together (peers are "rows with the same values for all ORDER BY expressions", windowfunctions.html
      2.2.1, which PARTITION BY groups the same way, NULLs together), when the frame holds every peer
      of the current row (a RANGE or GROUPS frame that holds it, bounded by peer groups, or ROWS from
      UNBOUNDED PRECEDING to UNBOUNDED FOLLOWING); EXCLUDE TIES is GROUP's with the current row's
      own count given back. Integer arithmetic, so exact.
    - min, max, first_value and last_value with EXCLUDE CURRENT ROW on a ROWS frame that holds the
      current row: the frame's two parts around it (_around()).
    Every other function (sum, total and avg, whose floating-point sums depend on the order they add
    in; nth_value, whose N would move with the part before's size), and EXCLUDE GROUP or TIES on
    anything but count(): not expressible."""
    from sqlglot import exp

    spec = window.args.get("spec")
    kind = str(spec.args.get("kind") or "").upper()
    exclude = " ".join(spec.args["exclude"].name.upper().split())
    lo = _offset(spec.args.get("start"), spec.args.get("start_side"))
    end = spec.args.get("end")
    hi = _offset(end, spec.args.get("end_side")) if end is not None else 0
    why = f"not expressible in {dialect}: a window frame's EXCLUDE {exclude} ({dialect} has no EXCLUDE"
    if lo is None or hi is None:
        raise NotExpressible(why + ", and the frame's bounds aren't literal)")
    holds = lo <= 0 <= hi
    if exclude == "CURRENT ROW" and kind == "ROWS" and not holds:
        spec.set("exclude", None)
        return window
    func = window.this
    if exclude == "CURRENT ROW" and kind == "ROWS" and holds and isinstance(func, (exp.Min, exp.Max, exp.FirstValue, exp.LastValue)):
        return _around(window, lo, hi)
    if not isinstance(func, exp.Count) or isinstance(func.this, exp.Distinct) or not holds:
        raise NotExpressible(why + "; only count(), min(), max(), first_value() and last_value() over a frame holding the current row have an exact rewrite)")
    all_peers = kind in ("RANGE", "GROUPS") or (lo == float("-inf") and hi == float("inf"))
    if exclude in ("GROUP", "TIES") and not all_peers:
        raise NotExpressible(why + "; a ROWS frame may hold only some of the current row's peers)")
    spec.set("exclude", None)
    arg = func.this
    own = exp.Literal.number(1) if isinstance(arg, exp.Star) or arg is None else exp.Case(ifs=[exp.If(this=exp.Is(this=arg.copy(), expression=exp.Null()), true=exp.Literal.number(0))], default=exp.Literal.number(1))
    whole = window.copy()
    if exclude == "CURRENT ROW":
        return exp.Paren(this=exp.Sub(this=whole, expression=own))
    order = window.args.get("order")
    terms = [o.this.copy() for o in (order.expressions if order else [])]
    peers = exp.Window(this=func.copy(), partition_by=[p.copy() for p in window.args.get("partition_by") or []] + terms)
    if exclude == "GROUP":
        return exp.Paren(this=exp.Sub(this=whole, expression=peers))
    return exp.Paren(this=exp.Add(this=exp.Sub(this=whole, expression=peers), expression=own))


# SQLite's storage classes ("Datatypes In SQLite" 2, https://www.sqlite.org/datatype3.html#storage_classes_and_datatypes):
# "INTEGER. The value is a signed integer, stored in 0, 1, 2, 3, 4, 6, or 8 bytes", "REAL. The value
# is a floating point value, stored as an 8-byte IEEE floating point number", "TEXT. The value is a
# text string", "BLOB. The value is a blob of data, stored exactly as it was input": each declared
# as the type that holds every value of the class whole (sqlglot writes each in each dialect).
STORAGE_TYPES = {"INTEGER": "BIGINT", "REAL": "DOUBLE", "TEXT": "TEXT", "BLOB": "BLOB"}

# What a value is when the file alone can't say which storage class it has (a value read from a
# column of mixed classes, a function's result, a subquery's).
UNKNOWN = "?"


class Table:
    """A table as a file defines it: its columns in order (upper-cased: SQLite's names are
    case-insensitive), each one's declared type (a sqlglot DataType; None for a column without a
    type, or an ANY column of a STRICT table: no affinity, values kept as written) and default, and,
    for each column without a type, the storage classes of the values the file writes to it."""

    def __init__(self, name):
        self.name, self.columns, self.kinds, self.defaults, self.written = name, [], {}, {}, {}

    def write(self, column, cls):
        if column in self.written and cls is not None:
            self.written[column].add(cls)

    def storage(self, column):
        """The storage class every value of `column` has, as far as the file has written it, else UNKNOWN."""
        kind = self.kinds.get(column, False)
        if kind is False:
            return UNKNOWN
        if kind is None:
            classes = self.written.get(column, set())
            return next(iter(classes)) if len(classes) == 1 else UNKNOWN
        return _declared_class(kind)


def _declared_class(kind):
    """The storage class of the values a column declared `kind` holds in SQLite and in every
    dialect alike: its affinity (affinity() above) where the dialect's type is of the same class,
    else UNKNOWN (NUMERIC affinity keeps 7 an INTEGER where DECIMAL makes it 7.00)."""
    from sqlglot import exp

    a = affinity(kind.sql(dialect="sqlite"))
    if a == "INTEGER" and kind.is_type(*exp.DataType.INTEGER_TYPES):
        return "INTEGER"
    if a == "REAL" and kind.is_type(*exp.DataType.REAL_TYPES) and not kind.is_type(exp.DataType.Type.DECIMAL):
        return "REAL"
    if a == "TEXT" and kind.is_type(*exp.DataType.TEXT_TYPES):
        return "TEXT"
    return UNKNOWN


class Context:
    """What translate() needs of the file around a record: the tables defined once it has run, and
    those it creates (by upper-cased name), their columns typed by what the whole file writes."""

    def __init__(self, tables=None, created=None):
        self.tables, self.created = tables or {}, created or {}


def _number_class(text, negative=False):
    """SQLite's "Literal Values" (https://www.sqlite.org/lang_expr.html#literal_values_constants_):
    "If a numeric literal has a decimal point or an exponentiation clause or if it is less than
    -9223372036854775808 or greater than 9223372036854775807, then it is a floating point literal.
    Otherwise it is an integer literal."
    """
    if not text.isdigit():
        return "REAL"
    return "INTEGER" if int(text) <= 2**63 - (0 if negative else 1) else "REAL"


def _sources(scope):
    """The tables a SELECT, UPDATE or DELETE reads, by the name its columns qualify them with (None
    for a source that isn't a table: a subquery, a CTE, a table function)."""
    from sqlglot import exp

    found = []
    if isinstance(scope, (exp.Update, exp.Delete)):
        found.append(scope.this)
    f = scope.args.get("from_") or scope.args.get("from")
    if isinstance(f, exp.From):
        found.append(f.this)
    found += [j.this for j in scope.args.get("joins") or []]
    out = {}
    for t in found:
        if isinstance(t, exp.Table) and isinstance(t.this, exp.Identifier):
            out[t.alias_or_name.upper()] = t.name.upper()
        else:
            out[getattr(t, "alias_or_name", "").upper() or id(t)] = None
    return out


def _column_class(column, tables):
    """The storage class of `column`'s values: from the table it names, or the one table of the
    nearest enclosing statement that has a column of its name; UNKNOWN when that's not one table."""
    from sqlglot import exp

    name, qualifier = column.name.upper(), column.table.upper()
    node = column.parent
    while node is not None:
        if isinstance(node, (exp.Select, exp.Update, exp.Delete)):
            sources = _sources(node)
            if qualifier:
                if qualifier in sources:
                    t = tables.get(sources[qualifier]) if sources[qualifier] else None
                    return t.storage(name) if t is not None else UNKNOWN
            else:
                if any(v is None for v in sources.values()):
                    return UNKNOWN
                having = [tables[v] for v in set(sources.values()) if v in tables and name in tables[v].kinds]
                if len(having) == 1:
                    return having[0].storage(name)
                if len(having) > 1 or any(v not in tables for v in sources.values()):
                    return UNKNOWN
        node = node.parent
    return UNKNOWN


def storage_classes(expression, tables):
    """Each node of `expression` (by id) with the storage class SQLite gives its value, or UNKNOWN,
    worked out children first over sqlglot's iterative walk (no recursion here): NULL is None (it
    fits every type); literals as lang_expr.html's "Literal Values" says (TRUE and FALSE are 1 and
    0); columns by their table (_column_class); `||` is TEXT; +, -, *, / and % of two INTEGERs are
    INTEGER and with a REAL REAL (lang_expr.html, "Operators": "integer arithmetic if both operands
    are integers and no overflow would result, or floating point arithmetic ... if either operand is
    a real value"); count() and length() INTEGER; sum(), min(), max() and abs() their argument's;
    avg() and total() REAL (lang_aggfunc.html); CAST by the type's affinity (lang_expr.html "CAST
    expressions"); anything else UNKNOWN."""
    from sqlglot import exp

    classes = {}
    nodes = list(expression.dfs())
    for node in reversed(nodes):
        c = UNKNOWN
        if isinstance(node, exp.Paren):
            c = classes.get(id(node.this), UNKNOWN)
        elif isinstance(node, exp.Null):
            c = None
        elif isinstance(node, exp.Boolean):
            c = "INTEGER"
        elif isinstance(node, exp.HexString):
            c = "BLOB"
        elif isinstance(node, exp.Literal):
            c = "TEXT" if node.is_string else _number_class(node.this)
        elif isinstance(node, exp.Neg):
            inner = node.this
            if isinstance(inner, exp.Literal) and not inner.is_string:
                c = _number_class(inner.this, negative=True)
            elif classes.get(id(inner)) in ("INTEGER", "REAL"):
                c = classes[id(inner)]
        elif isinstance(node, exp.Column):
            c = _column_class(node, tables)
        elif isinstance(node, exp.DPipe):
            c = "TEXT"
        elif isinstance(node, (exp.Add, exp.Sub, exp.Mul, exp.Div, exp.Mod, exp.IntDiv)):
            pair = {classes.get(id(node.this), UNKNOWN), classes.get(id(node.expression), UNKNOWN)}
            if pair == {"INTEGER"}:
                c = "INTEGER"
            elif pair <= {"INTEGER", "REAL"} and not isinstance(node, (exp.Mod, exp.IntDiv)):
                c = "REAL"
        elif isinstance(node, (exp.Count, exp.Length)):
            c = "INTEGER"
        elif isinstance(node, (exp.Sum, exp.Min, exp.Max, exp.Abs)) and not node.expressions:
            arg = classes.get(id(node.this), UNKNOWN)
            c = arg if arg in ("INTEGER", "REAL") else UNKNOWN
        elif isinstance(node, exp.Avg):
            c = "REAL"
        elif isinstance(node, exp.Cast) and isinstance(node.to, exp.DataType):
            c = _declared_class(node.to)
        classes[id(node)] = c
    return classes


def contexts(records):
    """Each record's Context, the file's records read in order as SQLite runs them: a CREATE TABLE
    defines its table (one of IF NOT EXISTS whose table is there leaves it), DROP TABLE removes it,
    and each INSERT (its VALUES or SELECT, its columns' defaults, its upsert's SET), UPDATE ... SET
    writes values to its columns. A `statement error` record SQLite refuses writes nothing."""
    import sqlglot
    from sqlglot import exp

    tables, out = {}, []
    for r in records:
        created = {}
        try:
            statements = sqlglot.parse(comment_closed(defaults_dropped(r.sql)), read=reader())
        except Exception:  # sqlglot can't read it: translate() says why
            statements = []
        ran = not (r.kind == "statement" and r.expect == "error")
        for e in statements:
            if isinstance(e, exp.Create) and e.args.get("kind") == "TABLE" and isinstance(e.this, exp.Schema) and isinstance(e.this.this, exp.Table):
                t = _table(e)
                if e.args.get("exists") and t.name in tables:
                    created[t.name] = tables[t.name]
                    continue
                created[t.name] = t
                if ran:
                    tables[t.name] = t
            elif not ran:
                continue
            elif isinstance(e, exp.Drop) and e.args.get("kind") == "TABLE" and isinstance(e.this, exp.Table):
                tables.pop(e.this.name.upper(), None)
            elif isinstance(e, (exp.Insert, exp.Update)):
                _writes(e, tables)
        out.append(Context(dict(tables), created))
    return out


def _table(create):
    from sqlglot import exp

    strict = any(True for _ in create.find_all(exp.StrictProperty))
    t = Table(create.this.this.name.upper())
    for c in create.this.expressions:
        if isinstance(c, exp.Identifier):
            name, kind, default = c.name.upper(), None, None
        elif isinstance(c, exp.ColumnDef):
            name, kind = c.name.upper(), c.args.get("kind")
            if strict and isinstance(kind, exp.DataType) and kind.meta.get("sqlite_any"):
                kind = None
            default = next((k.args["kind"].this for k in c.args.get("constraints") or [] if isinstance(k.args.get("kind"), exp.DefaultColumnConstraint)), None)
        else:
            continue
        t.columns.append(name)
        t.kinds[name], t.defaults[name] = kind, default
        if kind is None:
            t.written[name] = set()
    return t


def _writes(e, tables):
    """The values INSERT or UPDATE `e` writes, by storage class, into each untyped column."""
    from sqlglot import exp

    target = e.this
    names = None
    if isinstance(target, exp.Schema):
        names = [i.name.upper() for i in target.expressions]
        target = target.this
    t = tables.get(target.name.upper()) if isinstance(target, exp.Table) else None
    if t is None:
        return
    sets = []
    if isinstance(e, exp.Update):
        sets = list(e.expressions)
    else:
        names = names or t.columns
        source = e.expression
        if e.args.get("default"):
            rows = [[]]
        elif isinstance(source, exp.Values):
            rows = [list(row.expressions) if isinstance(row, exp.Tuple) else [row] for row in source.expressions]
        elif isinstance(source, exp.Select) and not any(isinstance(p, exp.Star) for p in source.selects):
            rows = [list(source.selects)]
        else:
            rows = None
        if rows is None:
            for n in names:
                t.write(n, UNKNOWN)
        else:
            for row in rows:
                for n, value in zip(names, row):
                    t.write(n, storage_classes(value, tables).get(id(value), UNKNOWN))
                for n in t.columns:
                    if n not in names and t.defaults.get(n) is not None:
                        d = t.defaults[n]
                        t.write(n, storage_classes(d, tables).get(id(d), UNKNOWN))
        conflict = e.args.get("conflict")
        if isinstance(conflict, exp.OnConflict):
            sets = list(conflict.expressions or [])
    for eq in sets:
        if isinstance(eq, exp.EQ) and isinstance(eq.this, exp.Column):
            t.write(eq.this.name.upper(), storage_classes(eq.expression, tables).get(id(eq.expression), UNKNOWN))


def typed(expression, context=None, dialect="the dialect"):
    """`expression` with each column declared without a type given the type of the values the
    file writes to it. SQLite's "Datatypes In SQLite", section 3.1 "Determination Of Column
    Affinity", rule 3: "If the declared type for a column contains the string "BLOB" or if no type
    is specified then the column has affinity BLOB" (https://www.sqlite.org/datatype3.html#determination_of_column_affinity);
    section 3: "A column with affinity BLOB does not prefer one storage class over another and no
    attempt is made to coerce data from one storage class into another." So such a column holds
    each value as written, and the engines that require a column's type (DuckDB, PostgreSQL, MySQL,
    ClickHouse) hold it the same only in a column of that value's type: where every value the file
    writes to it (contexts()) has one storage class, the column is declared that class's type
    (STORAGE_TYPES); where they're of several, or of one the file alone doesn't show, or there is
    none, no one type holds them as SQLite does: not expressible, with the reason. Never BLOB by
    default: a BLOB column of those engines converts or refuses what SQLite keeps.

    A STRICT table ("STRICT Tables", https://www.sqlite.org/stricttables.html) is one whose columns
    hold only their declared type: "SQLite attempts to coerce the data into the appropriate type ...
    If the value cannot be losslessly converted in the specified datatype, then an
    SQLITE_CONSTRAINT_DATATYPE error is raised", which is how every column of the four engines holds
    its type, so the table is declared without STRICT (DuckDB has no such word, and PostgreSQL, MySQL
    and ClickHouse refuse it); how each engine converts a value is its type system's, which the
    oracle classifies as such. Its ANY columns take any value unchanged ("no type coercion"), as a
    column without a type does: typed by their values, as above."""
    from sqlglot import exp

    context = context or Context()
    # sqlglot reads a column with neither type nor constraint as a bare name in CREATE TABLE's
    # schema, one with constraints only as a ColumnDef without a kind.
    for create in expression.find_all(exp.Create):
        schema = create.this
        if create.args.get("kind") == "TABLE" and isinstance(schema, exp.Schema):
            for column in list(schema.expressions):
                if isinstance(column, exp.Identifier):
                    column.replace(exp.ColumnDef(this=column.copy()))
            strict = [p for p in create.find_all(exp.StrictProperty)]
            for p in strict:
                p.pop()
            properties = create.args.get("properties")
            if strict and isinstance(properties, exp.Properties) and not properties.expressions:
                create.set("properties", None)
            if strict:
                for column in schema.find_all(exp.ColumnDef):
                    kind = column.args.get("kind")
                    if isinstance(kind, exp.DataType) and kind.meta.get("sqlite_any"):
                        column.set("kind", None)
    for column in expression.find_all(exp.ColumnDef):
        if column.args.get("kind") is not None:
            continue
        create = column.find_ancestor(exp.Create)
        name = create.this.this.name.upper() if create is not None and isinstance(create.this, exp.Schema) and isinstance(create.this.this, exp.Table) else ""
        table = context.created.get(name) or context.tables.get(name)
        where = f"column {column.name} of {name.lower() or 'the table'} has no type, so no affinity (https://www.sqlite.org/datatype3.html#determination_of_column_affinity), and SQLite keeps each value written to it as written"
        if table is None or column.name.upper() not in table.written:
            raise NotExpressible(f"not expressible in {dialect}: {where}; the values the file writes to it weren't read")
        classes = table.written[column.name.upper()] - {None}
        if UNKNOWN in classes:
            raise NotExpressible(f"not expressible in {dialect}: {where}; the file writes it a value whose storage class only running it shows")
        if not classes:
            raise NotExpressible(f"not expressible in {dialect}: {where}; the file writes it no value but NULL, so no one type is its")
        if len(classes) > 1:
            raise NotExpressible(f"not expressible in {dialect}: {where}; the file writes it values of several storage classes ({', '.join(sorted(classes))}), which no one {dialect} column type holds unchanged")
        column.set("kind", exp.DataType.build(STORAGE_TYPES[next(iter(classes))]))
    return expression


def divided(expression, context, dialect):
    """`expression` with each `/` whose operands are both INTEGER (storage_classes()) said as each
    dialect's integer division. SQLite's "Operators" (https://www.sqlite.org/lang_expr.html#operators_and_parse_affecting_attributes):
    "The other arithmetic operators perform integer arithmetic if both operands are integers and no
    overflow would result ... Integer divide yields an integer result, truncated toward zero."
    DuckDB's `/` always divides as floating point; its `//` is integer division ("Numeric Operators",
    https://duckdb.org/docs/stable/sql/functions/numeric: `//` "Integer division"), which truncates
    toward zero and gives NULL for a zero divisor as SQLite's does (DuckDB 1.4: -7 // 2 is -3, 7 // 0
    NULL). PostgreSQL's div(y, x) is the "integer quotient of y/x (truncates towards zero)" for any
    numeric operands (PostgreSQL manual, 9.3 "Mathematical Functions and Operators", Table 9.5),
    where its `/` keeps a fraction once an operand is numeric (sum() of a bigint is), and its divisor
    is given NULLIF(x, 0) as sqlglot gives `/`'s, for SQLite's NULL on a zero divisor. MySQL's
    `DIV` is "Integer division. Discards from the division result any fractional part to the right
    of the decimal point" (MySQL 8.4 manual, 14.6.1 "Arithmetic Operators"). ClickHouse is left as
    it is: its intDiv throws on a zero divisor ("Division by zero"), which SQLite's NULL isn't."""
    from sqlglot import exp

    if dialect not in ("duckdb", "postgres", "mysql"):
        return expression
    classes = storage_classes(expression, context.tables)
    whole = [d for d in reversed(list(expression.dfs())) if isinstance(d, exp.Div) and classes.get(id(d.this)) == "INTEGER" and classes.get(id(d.expression)) == "INTEGER"]
    for d in whole:
        divisor = d.expression
        if dialect == "postgres":
            divisor = exp.Nullif(this=divisor, expression=exp.Literal.number(0))
        replaced = exp.IntDiv(this=d.this, expression=divisor)
        if d is expression:
            expression = replaced
        else:
            d.replace(replaced)
    return expression


def keyed(expression, dialect):
    """`expression` with a table PRIMARY KEY's indexed columns said as each dialect can:
    - AUTOINCREMENT on the one column of the key: "CREATE TABLE t(x INTEGER, y, z, PRIMARY KEY(x
      ASC))" is the same table as "x INTEGER PRIMARY KEY" (lang_createtable.html, "ROWIDs and the
      INTEGER PRIMARY KEY"), so the key and its AUTOINCREMENT move onto the column, the form
      https://www.sqlite.org/autoinc.html describes, which sqlglot writes in each dialect;
    - COLLATE: the key's uniqueness compares by that collation while the column compares by its own
      (lang_createtable.html: an indexed column's collation is the index's; datatype3.html 7.1).
      BINARY on a column without a collation is the column's own and is left out; any other has no
      spelling in a dialect whose PRIMARY KEY names columns only (PostgreSQL's CREATE TABLE
      "PRIMARY KEY ( column_name [, ... ] )", MySQL's "PRIMARY KEY (key_part,...)", DuckDB's and
      ClickHouse's alike): not expressible."""
    from sqlglot import exp

    for key in list(expression.find_all(exp.PrimaryKey)):
        parts = key.expressions
        schema = key.find_ancestor(exp.Schema)
        columns = {c.name.upper(): c for c in schema.expressions if isinstance(c, exp.ColumnDef)} if schema else {}
        for part in parts:
            collate = part.meta.get("sqlite_collate")
            if collate is None:
                continue
            column = columns.get(part.name.upper())
            own = column is not None and not any(isinstance(c.args.get("kind"), exp.CollateColumnConstraint) for c in column.args.get("constraints") or [])
            if collate == "BINARY" and own:
                continue
            raise NotExpressible(f"not expressible in {dialect}: a PRIMARY KEY column's own collation (COLLATE {collate}, lang_createtable.html) has no spelling in {dialect}'s PRIMARY KEY, which names columns only")
        if any(p.meta.get("sqlite_autoincrement") for p in parts):
            column = columns.get(parts[0].name.upper()) if len(parts) == 1 else None
            if column is None:
                raise NotExpressible(f"not expressible in {dialect}: AUTOINCREMENT on a PRIMARY KEY that isn't one column of the table (https://www.sqlite.org/autoinc.html)")
            constraints = list(column.args.get("constraints") or [])
            constraints += [exp.ColumnConstraint(kind=exp.PrimaryKeyColumnConstraint()), exp.ColumnConstraint(kind=exp.AutoIncrementColumnConstraint())]
            column.set("constraints", constraints)
            (key.parent if isinstance(key.parent, exp.Constraint) else key).pop()
    return expression


# SQLite's built-in collations (https://www.sqlite.org/datatype3.html#collating_sequences, section
# 7.1: BINARY "compares string data using memcmp()", NOCASE "is the same as binary, except the 26
# upper case characters of ASCII are folded to their lower case equivalents", RTRIM "is the same as
# binary, except that trailing space characters are ignored") by the MySQL collation that compares as
# each does (MySQL 8.4 Reference Manual, 12.10.1 "Unicode Character Sets", which says: "For
# utf8mb4_0900_bin, the weight is the utf8mb4 encoding bytes", so memcmp's order on UTF-8; "The pad
# attribute for utf8mb4_bin is PAD SPACE, whereas for utf8mb4_0900_bin it is NO PAD", so utf8mb4_bin
# ignores trailing spaces; and 12.3.1 "Collation Naming Conventions": _as accent-sensitive, _ci
# case-insensitive). MySQL has no collation that folds ASCII's letters only: utf8mb4_0900_as_ci folds
# every letter's case, so NOCASE's answers differ there on letters outside ASCII, a difference of the
# engines, which the corpus shows as such. MySQL refuses the names it doesn't know ("Unknown
# collation: 'NOCASE'").
MYSQL_COLLATIONS = {"BINARY": "utf8mb4_0900_bin", "NOCASE": "utf8mb4_0900_as_ci", "RTRIM": "utf8mb4_bin"}


def spoken(expression, dialect):
    """`expression` with what sqlglot writes in SQLite's spelling, which `dialect` reads otherwise,
    given that dialect's spelling of it:
    - a blob literal X'hh..' (SQLite's "Literal Values", https://www.sqlite.org/lang_expr.html#literal_values_constants_:
      "BLOB literals are string literals containing hexadecimal data and preceded by a single "x" or
      "X" character") is a bit string in PostgreSQL's dialect (its manual, 4.1.2.5 "Bit-String
      Constants": "bit-string constants can be specified in hexadecimal notation, using a leading X
      (upper or lower case), e.g., X'1FF'"), which a BYTEA column refuses; PostgreSQL's bytea is
      written '\\xhh..' (8.4.1 "bytea Hex Format": "The entire string is preceded by the sequence
      \\x"), cast to BYTEA. OrioleDB and CedarDB speak PostgreSQL's dialect.
      ClickHouse: sqlglot refuses it ("Unsupported transpilation from BINARY/BLOB hex string");
      ClickHouse's unhex (its reference, "Encoding Functions": unhex "performs the opposite
      operation of hex. It interprets each pair of hexadecimal digits (in the argument) as a number
      and converts it to the byte represented by the number") gives the bytes as a String, the type a
      BLOB column is there.
    - a collation SQLite names (NOCASE, RTRIM, BINARY) is MySQL's that compares the same
      (MYSQL_COLLATIONS).
    - a window frame's EXCLUDE NO OTHERS, its default, is left out; its other exclusions, which
      MySQL and ClickHouse don't have, are said as excluded() says them, or the record is not
      expressible there (NotExpressible)."""
    from sqlglot import exp

    # A window frame's `EXCLUDE NO OTHERS` is its default (SQLite's "Window Functions", 2.2.4 "The
    # EXCLUDE clause": "the default is EXCLUDE NO OTHERS", as in standard SQL), which MySQL and
    # ClickHouse, that have no EXCLUDE, do unasked: it is left out.
    for frame in expression.find_all(exp.WindowSpec):
        exclude = frame.args.get("exclude")
        if isinstance(exclude, exp.Var) and " ".join(exclude.name.upper().split()) == "NO OTHERS":
            frame.set("exclude", None)
    if dialect == "mysql":
        # MySQL has no FULL JOIN (MySQL 8.4 Reference Manual, 15.2.13.2 "JOIN Clause": its
        # joined_table takes INNER, CROSS, LEFT, RIGHT and NATURAL [LEFT|RIGHT] joins only); sqlglot
        # says a FULL JOIN ON a condition as a LEFT and a RIGHT join, which a NATURAL join, whose
        # columns come from the tables' schemas (lang_select.html 2.2), can't give it.
        for join in expression.find_all(exp.Join):
            if str(join.args.get("side") or "").upper() == "FULL" and str(join.args.get("method") or "").upper() == "NATURAL":
                raise NotExpressible("not expressible in mysql: a NATURAL FULL JOIN (MySQL has no FULL JOIN, and its LEFT and RIGHT rewrite needs the join's columns, which a NATURAL join takes from the tables' schemas)")
    if dialect in ("mysql", "clickhouse"):
        for window in list(expression.find_all(exp.Window)):
            spec = window.args.get("spec")
            if isinstance(spec, exp.WindowSpec) and spec.args.get("exclude") is not None:
                said = excluded(window, dialect)
                if said is not window:
                    window.replace(said)
    if dialect == "duckdb" and any(True for _ in expression.find_all(exp.AutoIncrementColumnConstraint)):
        # AUTOINCREMENT: a new row's key is one more than the largest the table has ever held, an
        # explicitly written one included (https://www.sqlite.org/autoinc.html, "The AUTOINCREMENT
        # Keyword"). DuckDB's CREATE TABLE has no AUTOINCREMENT or identity column; its sequences
        # (CREATE SEQUENCE, a column's DEFAULT nextval()) count on their own and don't move past a
        # key written explicitly, so the next key differs: not expressible.
        raise NotExpressible("not expressible in duckdb: AUTOINCREMENT (https://www.sqlite.org/autoinc.html: one more than the largest key the table has held), which DuckDB's CREATE TABLE has no spelling of and its sequences don't count the same")
    if dialect == "clickhouse" and any(True for _ in expression.find_all(exp.AutoIncrementColumnConstraint)):
        # ClickHouse's CREATE TABLE has no AUTO_INCREMENT (its column expressions are DEFAULT,
        # MATERIALIZED, EPHEMERAL and ALIAS: https://clickhouse.com/docs/sql-reference/statements/create/table),
        # and generateSerialID() (25.1 on, a Keeper counter: "Generates and returns sequential numbers
        # starting from the previous counter value") counts on its own, past no key written
        # explicitly and back on no failed insert, so the next key differs: not expressible.
        raise NotExpressible("not expressible in clickhouse: AUTOINCREMENT (https://www.sqlite.org/autoinc.html: one more than the largest key the table has held), which ClickHouse's CREATE TABLE has no spelling of and its generateSerialID() counter doesn't count the same")
    if dialect in ("postgres", "clickhouse"):
        for blob in list(expression.find_all(exp.HexString)):
            digits = blob.this or ""
            if dialect == "postgres":
                blob.replace(exp.cast(exp.Literal.string("\\x" + digits), exp.DataType.build("BLOB")))
            else:
                blob.replace(exp.Unhex(this=exp.Literal.string(digits)))
    if dialect == "mysql":
        # sqlglot holds an expression's collation as Collate's `expression`, a column's as its
        # CollateColumnConstraint's `this`.
        for node, arg in [(c, "expression") for c in expression.find_all(exp.Collate)] + [(c, "this") for c in expression.find_all(exp.CollateColumnConstraint)]:
            name = node.args.get(arg)
            mysql = MYSQL_COLLATIONS.get(name.name.upper()) if isinstance(name, (exp.Var, exp.Column, exp.Identifier)) else None
            if mysql:
                node.set(arg, exp.Var(this=mysql))
    return expression


# Clauses SQLite reads that state what every engine the corpus compares does unasked, so the record
# means the same with them left out (each by SQLite's documentation):
# - `OR ABORT` after INSERT or UPDATE, and `ON CONFLICT ABORT` on a constraint: "ABORT ... is the
#   default behavior" (https://www.sqlite.org/lang_conflict.html), which is what PostgreSQL, MySQL,
#   DuckDB and ClickHouse do on a violated constraint: the statement fails and its changes are undone.
# - `NOT DEFERRABLE`, with or without `INITIALLY IMMEDIATE` or `INITIALLY DEFERRED`: "NOT DEFERRABLE
#   INITIALLY DEFERRED ... An immediate foreign key constraint" and immediate "is the default"
#   (https://www.sqlite.org/foreignkeys.html, section 4.2), PostgreSQL's default too (its manual,
#   CREATE TABLE: "NOT DEFERRABLE is the default").
# - `MATCH <name>` after a foreign key's REFERENCES: SQLite "parses MATCH clauses ... but does not
#   enforce them. All foreign key constraints in SQLite are handled as if MATCH SIMPLE were
#   specified" (foreignkeys.html, section 4.3); MATCH SIMPLE is PostgreSQL's default (CREATE TABLE:
#   "MATCH SIMPLE ... is the default") and MySQL parses MATCH and ignores it.
# sqlglot's SQLite reader refuses each of them, so they are taken out of the text before it reads it,
# by SQLite's own tokens (a string or a quoted name is never touched).
_REFERENCES_TAIL = {"ON", "DELETE", "UPDATE", "SET", "NULL", "DEFAULT", "CASCADE", "RESTRICT", "NO", "ACTION", ",", "(", ")", "."}


def defaults_dropped(sql):
    """`sql` without the clauses above, or as it is when SQLite's tokenizer can't read it."""
    from sqlglot.dialects.sqlite import SQLite
    from sqlglot.tokens import TokenType

    try:
        tokens = SQLite().tokenize(sql)
    except Exception:  # sqlglot can't tokenize it: translate() says why
        return sql
    quoted = (TokenType.STRING, TokenType.IDENTIFIER, TokenType.HEX_STRING, TokenType.BIT_STRING, TokenType.NATIONAL_STRING)
    words = [None if t.token_type in quoted else t.text.upper() for t in tokens]
    names = [t.token_type == TokenType.VAR or (t.token_type not in quoted and t.text.isidentifier()) for t in tokens]

    def after_references(i):
        # The nearest REFERENCES before `i`, with only its table, columns and ON actions between.
        for j in range(i - 1, -1, -1):
            if words[j] == "REFERENCES":
                return True
            if words[j] not in _REFERENCES_TAIL and not names[j]:
                return False
        return False

    drop, i = [], 0
    while i < len(words):
        w = words[i : i + 4] + [None] * 3
        n = 0
        if w[:2] == ["OR", "ABORT"] and i > 0 and words[i - 1] in ("INSERT", "UPDATE"):
            n = 2
        elif w[:3] == ["ON", "CONFLICT", "ABORT"]:
            n = 3
        elif w[:2] == ["NOT", "DEFERRABLE"]:
            n = 4 if w[2] == "INITIALLY" and w[3] in ("IMMEDIATE", "DEFERRED") else 2
        elif w[0] == "MATCH" and i + 1 < len(tokens) and names[i + 1] and after_references(i):
            n = 2
        if n:
            drop.append((tokens[i].start, tokens[i + n - 1].end + 1))
            i += n
        else:
            i += 1
    if not drop:
        return sql
    out, at = [], 0
    for start, end in drop:
        out.append(sql[at:start])
        at = end
    out.append(sql[at:])
    return "".join(out)


def translate(sql, dialect, context=None):
    """`sql`, in SQLite's dialect, in `dialect`: (text, None), or (None, why) when sqlglot can't say
    it there or the dialect has nothing that means the same. `context` is the file around it
    (contexts()): its tables, for the types of untyped columns and integer division; without it a
    column without a type is not expressible. The one translation the corpus's tools share (probe.py's too)."""
    if dialect == "sqlite":
        return sql, None
    import sqlglot

    text = comment_closed(defaults_dropped(sql))
    clause = conflict_clause(text)
    if clause is not None:
        return None, f"not expressible in {dialect}: SQLite's conflict resolution {clause} (https://www.sqlite.org/lang_conflict.html) has no equivalent there"
    if upserts(text) > 1:
        # SQLite's "UPSERT" (https://www.sqlite.org/lang_upsert.html): "there can be multiple upsert
        # clauses", each tried in turn. PostgreSQL's and DuckDB's INSERT take one ON CONFLICT clause,
        # MySQL's ON DUPLICATE KEY UPDATE acts on any key, ClickHouse has no upsert.
        return None, f"not expressible in {dialect}: several upsert clauses (https://www.sqlite.org/lang_upsert.html) where {dialect} takes at most one"
    try:
        # sqlglot.transpile's steps, each statement given its untyped columns' types between them.
        out = []
        for e in sqlglot.parse(text, read=reader()):
            if not e:
                out.append("")
                continue
            e = spoken(keyed(typed(divided(e, context or Context(), dialect), context, dialect), dialect), dialect)
            after = autoincremented(e) if dialect == "postgres" else []
            out += [e.sql(dialect=dialect, unsupported_level=sqlglot.ErrorLevel.RAISE)] + after
    except NotExpressible as e:
        return None, str(e)
    except Exception as e:  # sqlglot can't say it in this dialect
        return None, f"untranslatable: {type(e).__name__}: {str(e)[:200]}"
    if dialect == "clickhouse":
        # A primary key orders a MergeTree table, and its columns can't be Nullable there.
        out = [re.sub(r"Nullable\(([A-Za-z0-9]+(?:\([0-9, ]+\))?)\)(\s+PRIMARY KEY)", r"\1\2", o) for o in out]
    if dialect == "mysql":
        # SQLite's TEXT column is MySQL's VARCHAR: MySQL's TEXT takes no DEFAULT and no index
        # without a prefix length, which SQLite's text has no need of.
        out = [re.sub(r"\bTEXT\b", "VARCHAR(255)", o) if re.match(r"\s*(CREATE\s+(TEMPORARY\s+)?TABLE|ALTER\s+TABLE)", o, re.I) else o for o in out]
        # SQLite's BLOB is MySQL's BLOB too: sqlglot writes it VARBINARY, which MySQL takes only
        # with a length.
        out = [re.sub(r"\bVARBINARY\b(?!\s*\()", "BLOB", o) if re.match(r"\s*(CREATE\s+(TEMPORARY\s+)?TABLE|ALTER\s+TABLE)", o, re.I) else o for o in out]
    if not out:
        return None, "untranslatable: sqlglot wrote nothing"
    return ";\n".join(out), None


def autoincremented(expression):
    """`expression` (in PostgreSQL's dialect) with each AUTOINCREMENT column's keys given as SQLite
    gives them, and the statements that do it after it. SQLite's "AUTOINCREMENT"
    (https://www.sqlite.org/autoinc.html): a new row's key is "one larger than the largest ROWID
    that has ever before existed in that same table", kept in the table sqlite_sequence, which
    "is updated ... whenever a normal table that contains an AUTOINCREMENT column is created"
    and written with each insert, in the inserting statement's transaction; a key written
    explicitly counts too, and NULL is "the next" key. sqlglot writes it as an identity column,
    whose sequence moves past neither an explicitly written key nor back on a failed or
    rolled-back insert (PostgreSQL's manual, 9.17 "Sequence Manipulation Functions": "nextval
    operations are never rolled back"), so its next key can differ. So the key is given as SQLite
    gives it: a table `<table>_autoincrement` holds the largest key the table has held (as
    sqlite_sequence does, transactional as it is), and a BEFORE INSERT trigger (PostgreSQL's
    CREATE TRIGGER, its PL/pgSQL "Triggers on Data Changes": "a row-level trigger fired before an
    operation ... can return NEW" changed) gives a row without a key one more than the larger of
    that and the table's largest key, then records the row's key there. Its UPDATE runs in the
    inserting statement, so a refused insert leaves it as it was, as SQLite's does. A CREATE TABLE
    without IF NOT EXISTS starts a fresh count (a table dropped and made again forgets its keys:
    autoinc.html, "the sqlite_sequence row for an AUTOINCREMENT table is automatically deleted when
    the table is dropped"). A TEMPORARY table's would need the function in pg_temp, which a
    trigger can't name without its schema: not expressible."""
    from sqlglot import exp

    out = []
    for create in list(expression.find_all(exp.Create)):
        schema = create.this
        if create.args.get("kind") != "TABLE" or not isinstance(schema, exp.Schema) or not isinstance(schema.this, exp.Table):
            continue
        for column in [c for c in schema.expressions if isinstance(c, exp.ColumnDef)]:
            auto = [c for c in column.args.get("constraints") or [] if isinstance(c.args.get("kind"), exp.AutoIncrementColumnConstraint)]
            if not auto:
                continue
            properties = create.args.get("properties")
            if properties is not None and any(isinstance(p, exp.TemporaryProperty) for p in properties.expressions):
                raise NotExpressible("not expressible in postgres: AUTOINCREMENT on a TEMPORARY table (https://www.sqlite.org/autoinc.html), whose trigger function would be pg_temp's")
            for c in auto:
                c.pop()
            table = schema.this
            side = table.copy()
            side.set("this", exp.Identifier(this=table.name + "_autoincrement", quoted=table.this.quoted))
            t, s, k = table.sql(dialect="postgres"), side.sql(dialect="postgres"), column.this.sql(dialect="postgres")
            body = (
                f"BEGIN IF NEW.{k} IS NULL THEN NEW.{k} := greatest((SELECT max({k}) FROM {t}), (SELECT seq FROM {s})) + 1; END IF; "
                f"UPDATE {s} SET seq = greatest(seq, NEW.{k}); RETURN NEW; END"
            )
            if not create.args.get("exists"):
                out.append(f"DROP TABLE IF EXISTS {s}")
            out += [
                f"CREATE TABLE IF NOT EXISTS {s} (seq BIGINT NOT NULL)",
                f"INSERT INTO {s} SELECT 0 WHERE NOT EXISTS (SELECT 1 FROM {s})",
                "CREATE OR REPLACE FUNCTION " + s + "() RETURNS trigger LANGUAGE plpgsql AS '" + body.replace("'", "''") + "'",
                f"CREATE OR REPLACE TRIGGER {s} BEFORE INSERT ON {t} FOR EACH ROW EXECUTE FUNCTION {s}()",
            ]
    return out


def record_in(record, dialect, context=None):
    """`record` (slt.Record) in `dialect`, as translate() says it. A record SQLite refuses
    (`statement error`) that sqlglot can't read either is given as SQLite holds it, its default
    clauses left out: what the record asks of an engine is that it refuse the statement too, and an
    engine that can't read it refuses it (the corpus's records, as sqllogictest's, don't say which
    error). One sqlglot can read is translated, so an engine that refuses it does so for the
    statement's sake, not its spelling."""
    sql, why = translate(record.sql, dialect, context)
    if why is not None and record.kind == "statement" and record.expect == "error":
        return defaults_dropped(record.sql), None
    return sql, why


def refusals(pool, rel):
    """SQLite's error for each `statement error` record of `<pool>/<rel>` (run as the corpus's
    runner runs it, after its fixture `<pool>/<source>/base.test` when it has one): a JSON-ready
    dict each, its line, SQLite's extended result code and name (Python's sqlite3.Error
    `sqlite_errorcode` and `sqlite_errorname`, Python 3.11 on: None before) and its message, which
    the harness (crates/corpus/src/refusal.rs) reads for the error's kind."""
    path = os.path.join(pool, rel)
    fixture = os.path.join(pool, rel.split(os.sep)[0], "base.test")
    kept = []

    def refused(rec, e):
        kept.append({"line": rec.line, "code": getattr(e, "sqlite_errorcode", None), "name": getattr(e, "sqlite_errorname", None), "message": str(e)})

    slt.run_file(path, fixture=fixture if os.path.exists(fixture) and os.path.abspath(fixture) != os.path.abspath(path) else None, refused=refused)
    return kept


def files(pool):
    for root, _dirs, names in os.walk(pool, followlinks=True):
        if os.path.relpath(root, pool).split(os.sep)[0] in ("translations", "refusals"):
            continue
        for n in sorted(names):
            if n.endswith(".test"):
                yield os.path.relpath(os.path.join(root, n), pool)


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    pool = sys.argv[1]
    dialects = sys.argv[2:] or DIALECTS
    counts = {d: [0, 0] for d in dialects}
    refused = 0
    for rel in files(pool):
        records = [r for kind, r in slt.parse(os.path.join(pool, rel)) if kind == "record"]
        around = contexts(records)
        for d in dialects:
            out = os.path.join(pool, "translations", d, rel + ".jsonl")
            os.makedirs(os.path.dirname(out), exist_ok=True)
            with open(out, "w", encoding="utf-8") as f:
                for r, c in zip(records, around):
                    sql, why = record_in(r, d, c)
                    counts[d][why is not None] += 1
                    f.write(json.dumps({"line": r.line, "sql": sql} if why is None else {"line": r.line, "untranslatable": why}) + "\n")
        # SQLite's error for each record it refuses, for comparing refusals by kind.
        out = os.path.join(pool, "refusals", rel + ".jsonl")
        os.makedirs(os.path.dirname(out), exist_ok=True)
        kept = refusals(pool, rel)
        with open(out, "w", encoding="utf-8") as f:
            for k in kept:
                f.write(json.dumps(k) + "\n")
        refused += len(kept)
    for d, (ok, no) in counts.items():
        print(f"{d}: {ok} records translated, {no} untranslatable")
    print(f"sqlite: {refused} refusals kept (pool/refusals)")


if __name__ == "__main__":
    main()
