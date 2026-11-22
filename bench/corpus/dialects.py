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
        PRIMARY_PARSERS = {**SQLite.Parser.PRIMARY_PARSERS, TokenType.HEX_STRING: lambda self, token: self._parse_hex(token)}
        UNARY_PARSERS = {**SQLite.Parser.UNARY_PARSERS, TokenType.DASH: lambda self: self._parse_negated()}

        def _parse_hex(self, token):
            # 0x10 is an integer, X'10' a blob: sqlglot's SQLite tokenizer makes both one HEX_STRING
            # token of the digits alone, so the text it was read from tells them apart. SQLite's
            # "Literal Values" (https://www.sqlite.org/lang_expr.html#literal_values_constants_):
            # "Hexadecimal integer literals follow the C-language notation of "0x" or "0X" followed
            # by hexadecimal digits. For example, 0x1234 means the same as 4660 and
            # 0x8000000000000000 means the same as -9223372036854775808. Hexadecimal literals are
            # interpreted as 64-bit two's-complement integers and are thus limited to sixteen
            # significant digits of precision." So it is written as the decimal integer it means;
            # one of more than sixteen significant digits SQLite refuses ("hex literal too big").
            if self.sql[token.start : token.start + 2].upper() != "0X":
                return SQLite.Parser.PRIMARY_PARSERS[TokenType.HEX_STRING](self, token)
            value = int(token.text, 16)
            if value >= 2**64:
                self.raise_error(f"hex literal too big: 0x{token.text} (SQLite takes at most sixteen significant hexadecimal digits, lang_expr.html)", token)
            value = value - 2**64 if value >= 2**63 else value
            said = exp.Literal.number(value)
            said.meta["sqlite_hex"] = value
            return said

        def _parse_negated(self):
            # A minus before a hex literal (through parentheses, which SQLite's parser drops: "expr
            # ::= LP expr RP", src/parse.y) negates its 64-bit value, and SQLite refuses the one
            # value whose negation has none, -0x8000000000000000 ("hex literal too big", src/expr.c
            # codeInteger), rather than make it a floating point value as it does a decimal one.
            negated = self.expression(exp.Neg(this=self._parse_unary()))
            inner = negated.this
            while isinstance(inner, exp.Paren):
                inner = inner.this
            if inner is not None and inner.meta.get("sqlite_hex") == -(2**63):
                self.raise_error("hex literal too big: -0x8000000000000000 (its negation is no 64-bit integer, and SQLite refuses it)")
            return negated
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
    those it creates (by upper-cased name), their columns typed by what the whole file writes; and
    whether PRAGMA case_sensitive_like is on when it runs (liked())."""

    def __init__(self, tables=None, created=None, case_sensitive_like=False):
        self.tables, self.created, self.case_sensitive_like = tables or {}, created or {}, case_sensitive_like


def _pragma_set(e, name):
    """The boolean `PRAGMA [schema.]<name> = value` sets, or None when `e` isn't one. SQLite's
    "PRAGMA Statements" (https://www.sqlite.org/pragma.html): "A boolean value ... 1 yes true on
    ... 0 no false off"."""
    from sqlglot import exp

    if not isinstance(e, exp.Pragma) or not isinstance(e.this, exp.EQ):
        return None
    target, value = e.this.this, e.this.expression
    if (target.expression if isinstance(target, exp.Dot) else target).name.lower() != name:
        return None
    if isinstance(value, exp.Boolean):
        return value.this
    word = value.name.lower() if isinstance(value, (exp.Literal, exp.Var, exp.Column, exp.Identifier)) else ""
    return True if word in ("1", "yes", "true", "on") else False if word in ("0", "no", "false", "off") else None


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
    a real value"); count() and length() INTEGER; sum(), min(), max() and abs() their argument's (max() and min() of several arguments theirs when they share one, NULL with a NULL);
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
        elif isinstance(node, (exp.Min, exp.Max)):
            # max(X, Y, ...) and min(): one of its arguments, NULL if any is (lang_corefunc.html#max_scalar).
            args = {classes.get(id(a), UNKNOWN) for a in [node.this] + list(node.expressions)}
            c = None if None in args else next(iter(args)) if len(args) == 1 and args <= {"INTEGER", "REAL"} else UNKNOWN
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

    tables, out, case_sensitive_like = {}, [], False
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
            elif _pragma_set(e, "case_sensitive_like") is not None:
                case_sensitive_like = _pragma_set(e, "case_sensitive_like")
        out.append(Context(dict(tables), created, case_sensitive_like))
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


_ASCII_UPPER, _ASCII_LOWER = "ABCDEFGHIJKLMNOPQRSTUVWXYZ", "abcdefghijklmnopqrstuvwxyz"


def liked(expression, context, dialect):
    """`expression` with each LIKE said as SQLite's. SQLite's "The LIKE, GLOB, REGEXP, MATCH, and
    extract operators" (https://www.sqlite.org/lang_expr.html#the_like_glob_regexp_match_and_extract_operators):
    - "Any other character matches itself or its lower/upper case equivalent (i.e. case-insensitive
      matching). Important Note: SQLite only understands upper/lower case for ASCII characters by
      default", unless PRAGMA case_sensitive_like is on (pragma.html#pragma_case_sensitive_like),
      where DuckDB's, PostgreSQL's and ClickHouse's LIKE are case-sensitive always. So there, unless
      the pragma is on, both sides are given ASCII's case folded and nothing else: translate(x,
      'A..Z', 'a..z') (PostgreSQL's manual, 9.4 "String Functions": translate "Replaces each
      character in string that matches a character in the from set with the corresponding
      character in the to set"; DuckDB's and ClickHouse's translate the same). An ESCAPE that is an
      ASCII letter would be folded with the pattern, so means otherwise: not expressible. MySQL's
      LIKE compares by its collation, case-insensitively by default (utf8mb4_0900_ai_ci, which also
      folds accents and letters beyond ASCII): left as it is, a difference of the engine, as
      MYSQL_COLLATIONS' NOCASE is.
    - "the optional ESCAPE clause ... If the ESCAPE clause is used, the escape character ..."; without
      one there is none, where PostgreSQL's and MySQL's LIKE take a backslash as one by default
      (PostgreSQL 9.7.1: "The default escape character is the backslash but a different one can be
      selected by using the ESCAPE clause. It's also possible to select no escape character by
      writing ESCAPE ''"; MySQL 8.4 14.8.1: "The escape sequence should be one character long to
      specify the escape character, or empty to specify that no escape character is used"), as
      ClickHouse's does with no ESCAPE clause at all (its "String Search Functions", like: "use the
      backslash (\\) for escaping metasymbols"). So PostgreSQL and MySQL are given ESCAPE '', and
      ClickHouse the pattern with each backslash doubled (replaceAll). DuckDB's LIKE has no default
      escape character, as SQLite's."""
    from sqlglot import exp

    def fold(e):
        return exp.Anonymous(this="translate", expressions=[e, exp.Literal.string(_ASCII_UPPER), exp.Literal.string(_ASCII_LOWER)])

    for like in list(expression.find_all(exp.Like)):
        escape = like.parent if isinstance(like.parent, exp.Escape) and like.parent.this is like else None
        if dialect in ("duckdb", "postgres", "clickhouse") and not context.case_sensitive_like:
            mark = escape.expression if escape is not None else None
            if mark is not None and not (isinstance(mark, exp.Literal) and mark.is_string and not (len(mark.this) == 1 and mark.this in _ASCII_UPPER + _ASCII_LOWER)):
                raise NotExpressible(f"not expressible in {dialect}: LIKE with an ESCAPE that is an ASCII letter or not a string literal, which folding the pattern's case (SQLite's LIKE is case-insensitive for ASCII, lang_expr.html) would change")
            like.set("this", fold(like.this))
            like.set("expression", fold(like.expression))
        if escape is not None:
            continue
        if dialect in ("postgres", "mysql"):
            said = exp.Escape(this=like.copy(), expression=exp.Literal.string(""))
            if like is expression:
                expression = said
            else:
                like.replace(said)
        elif dialect == "clickhouse":
            like.set("expression", exp.Anonymous(this="replaceAll", expressions=[like.expression, exp.Literal.string("\\"), exp.Literal.string("\\\\")]))
    return expression


def glob_regex(pattern, dialect):
    """SQLite's GLOB pattern `pattern` as a regular expression matching the whole string the same,
    or NotExpressible. SQLite's GLOB (lang_expr.html#the_like_glob_regexp_match_and_extract_operators:
    "The GLOB operator is similar to LIKE but uses the Unix file globbing syntax for its wildcards.
    Also, GLOB is case sensitive, unlike LIKE"), as its matcher reads it (src/func.c,
    patternCompare(), https://sqlite.org/src/file?name=src/func.c): `*` any characters, `?` one,
    `[...]` one of a set (`^` first negates it, `]` first is itself, `a-z` a range of code points,
    `-` first or last itself), every other character itself, a backslash too (GLOB has no escape
    character). The expression's `.` takes a newline as any other character ((?s) for RE2 and ICU;
    PostgreSQL's does unasked, its manual 9.7.3.5), and it is anchored at both ends (MySQL's ICU `$`
    also matches before a final newline, so `\z` there). Only the characters a regular expression
    reads otherwise are given a backslash (RE2 refuses one before some others)."""
    out, i, n = [], 0, len(pattern)

    def lit(c):
        return "\\" + c if c in "\\.^$|?*+()[]{}-" else c

    while i < n:
        c = pattern[i]
        if c == "*":
            out.append(".*")
        elif c == "?":
            out.append(".")
        elif c == "[":
            # prior: the set's last single character, which a `-` makes a range's start; none
            # after a range (patternCompare()'s prior_c, set to 0 there).
            j, negate, members, prior = i + 1, False, [], None
            if j < n and pattern[j] == "^":
                negate, j = True, j + 1
            if j < n and pattern[j] == "]":
                members.append(lit("]"))
                prior, j = "]", j + 1
            while j < n and pattern[j] != "]":
                if pattern[j] == "-" and prior is not None and j + 1 < n and pattern[j + 1] != "]":
                    hi = pattern[j + 1]
                    if ord(hi) < ord(prior):
                        raise NotExpressible(f"not expressible in {dialect}: a GLOB range [{prior}-{hi}] whose end comes before its start, which matches nothing in SQLite and which regular expressions refuse")
                    members[-1] = f"{lit(prior)}-{lit(hi)}"
                    prior, j = None, j + 2
                    continue
                members.append(lit(pattern[j]))
                prior, j = pattern[j], j + 1
            if j >= n:
                raise NotExpressible(f"not expressible in {dialect}: a GLOB pattern with a '[' not closed, which SQLite's matcher stops at")
            out.append("[" + ("^" if negate else "") + "".join(members) + "]")
            i = j
        else:
            out.append(lit(c))
        i += 1
    body = "".join(out)
    if dialect == "postgres":
        return "^" + body + "$"
    return "(?s)^" + body + ("\\z" if dialect == "mysql" else "$")


def globbed(expression, dialect):
    """`expression` with each GLOB said as a regular expression match (glob_regex()), where sqlglot
    writes GLOB, which PostgreSQL, MySQL and ClickHouse don't have and DuckDB's reads otherwise
    (its GLOB negates a set with `!` and takes a backslash as an escape: DuckDB 1.4.2 gives
    'bc' GLOB '[^a]*' false, SQLite true). Each engine's match: PostgreSQL's `~` (case-sensitive,
    its manual 9.7.3), DuckDB's regexp_matches(), MySQL's REGEXP_LIKE() with match type 'c'
    (case-sensitive: "c: Case-sensitive matching", MySQL 8.4 14.8.2, where its collation would
    otherwise fold case), ClickHouse's match(). A NULL pattern gives NULL as SQLite's does; a pattern
    that isn't a string literal is not expressible (its expression is made from its text)."""
    from sqlglot import exp

    for g in [n for n in reversed(list(expression.dfs())) if isinstance(n, exp.Glob)]:
        pattern = g.expression
        if isinstance(pattern, exp.Null):
            regex = exp.Null()
        elif isinstance(pattern, exp.Literal) and pattern.is_string:
            regex = exp.Literal.string(glob_regex(pattern.this, dialect))
        else:
            raise NotExpressible(f"not expressible in {dialect}: GLOB with a pattern that isn't a string literal (lang_expr.html), whose regular expression is made from its text")
        said = exp.RegexpLike(this=g.this.copy(), expression=regex, flag=exp.Literal.string("c") if dialect == "mysql" else None)
        if g is expression:
            expression = said
        else:
            g.replace(said)
    return expression


def _real_text(x, dialect, what):
    """REAL `x` as the text SQLite writes it, for hex() and length() of it: SQLite's real-to-text
    conversion is printf's "%!.15g" (sqlite3 3.51's vdbemem.c sqlite3VdbeMemStringify(); its "!"
    flag keeps a decimal point: 100.0 is '100.0', 1e20 '1.0e+20'), infinities 'Inf' and '-Inf',
    and -0.0 '0.0' (Python's sqlite3 3.51.0 gives each). DuckDB's printf() takes %.15g, without
    the point: given back as regexp_replace() of a mantissa without one, and its 'inf' as 'Inf';
    x is read once. One difference stays: a real whose decimal expansion ties at the fifteenth
    digit (100000000000000.5) SQLite's own printf rounds away from zero and DuckDB's to even
    ('100000000000001.0' against '100000000000000.0'). PostgreSQL's and MySQL's float text is the shortest that reads back (100.0 is
    '100', 0.1 + 0.2 '0.30000000000000004'), ClickHouse's too, and none has a %g: not
    expressible there."""
    from sqlglot import exp

    if dialect != "duckdb":
        raise NotExpressible(f"not expressible in {dialect}: {what}() of a REAL, which reads SQLite's text of the real (printf's %!.15g, lang_corefunc.html#{what}); {dialect} writes a real as the shortest text that reads back and has no %g")
    g = exp.Anonymous(this="printf", expressions=[exp.Literal.string("%.15g"), x.copy()])
    pointed = exp.RegexpReplace(this=g, expression=exp.Literal.string("^(-?[0-9]+)(e|$)"), replacement=exp.Literal.string("\\1.0\\2"))
    return exp.Anonymous(this="replace", expressions=[pointed, exp.Literal.string("inf"), exp.Literal.string("Inf")])


def functions_said(expression, context, dialect):
    """`expression` with SQLite's built-in functions that sqlglot writes as functions of the same
    name that mean otherwise, said as SQLite's (lang_corefunc.html, "Built-In Scalar SQL Functions"):
    - likely(X) and unlikely(X): "returns the argument X unchanged", a hint to the query planner;
      likelihood(X,Y): "returns argument X unchanged. The value Y in likelihood(X,Y) must be a
      floating point constant between 0.0 and 1.0, inclusive" (SQLite refuses an integer Y: "second
      argument to likelihood() must be a constant between 0.0 and 1.0"). Each is X, in parentheses;
      a likelihood() SQLite refuses is left as it is, for the engine to refuse. No other engine has
      them.
    - upper(X) and lower(X): "a copy of input string X in which all lower-case ASCII characters are
      converted to their upper-case equivalent" (lower the other way), ASCII's letters only, where
      DuckDB's and PostgreSQL's convert every letter: translate(X, 'a..z', 'A..Z') there (liked()'s
      translate). ClickHouse's upper and lower are ASCII's already ("String Functions": upperUTF8
      is its Unicode one); MySQL's convert by the character set's case mapping: left as it is, a
      difference of the engine.
    - round(X,Y): "If the Y argument is omitted or negative, it is taken to be 0", where every other
      engine rounds to tens, hundreds, ...: Y is given as CASE WHEN Y < 0 THEN 0 ELSE Y END (a
      literal Y as its value), so a Y of random() is not expressible.
    - char(X1,...,XN): "a string composed of characters having the unicode code point values of
      integers X1 through XN", a NULL being 0 (SQLite gives char(NULL) as one character U+0000),
      where sqlglot writes DuckDB's chr() of X1 alone: there, chr(coalesce(Xi, 0)) of each,
      joined with ||.
    - format(FORMAT, ...): "an alias for printf()" (lang_corefunc.html#format), where sqlglot keeps
      the name, which is another function in each engine (PostgreSQL's takes %s, %I and %L only;
      MySQL's FORMAT(X,D) formats a number; DuckDB's and ClickHouse's take {} fields): said as
      printf(), which DuckDB and ClickHouse have ("printf-style", their references) and PostgreSQL
      and MySQL refuse.
    - hex(X): "interprets its argument as a BLOB and returns a string which is the upper-case
      hexadecimal rendering of the content of that blob" (lang_corefunc.html#hex), so an integer is
      its text's bytes (hex(1) is '31') and NULL the empty blob's '' (SQLite 3.51 gives ''), where
      DuckDB's and MySQL's hex() of an integer give its value in base 16, ClickHouse's its bytes,
      and each gives NULL for NULL: on those three an INTEGER X (storage_classes()) is cast to text
      first, and the result is coalesce(hex(X), ''). A REAL X is SQLite's text of it (_real_text():
      DuckDB's printf(), not expressible on the others). PostgreSQL has no hex().
    - zeroblob(N): "a BLOB consisting of N bytes of 0x00" (lang_corefunc.html#zeroblob), none for
      a negative N, and SQLite 3.51 gives NULL's as the empty blob. DuckDB and PostgreSQL have no
      zeroblob(): for an INTEGER or NULL N (storage_classes()) it is DuckDB's repeat() of the one
      byte (unhex('00'), "repeat(string, count)" takes a BLOB there, none for a count below one)
      and PostgreSQL's decode(repeat('00', N), 'hex') (9.4 and 9.5: repeat gives '' for a count
      below one), N coalesced to 0. Another N, which SQLite takes as an integer its own way, is
      left as it is, for the engine to refuse.
    - length(X): "For a string value X, the length(X) function returns the number of characters
      ... If X is a blob, then length(X) returns the number of bytes in the blob ... If X is numeric
      then length(X) returns the length of a string representation of X" (lang_corefunc.html#length).
      DuckDB's, PostgreSQL's and ClickHouse's take no integer: an INTEGER X (storage_classes()),
      whose text is the same on every engine, is cast to text first; a REAL X is SQLite's text of
      it (_real_text(): DuckDB's printf(), not expressible on the others). A BLOB X is DuckDB's octet_length() (its length() takes no BLOB) and
      ClickHouse's length(), its bytes, where sqlglot writes CHAR_LENGTH, its UTF-8 characters.
      PostgreSQL's length() of a bytea and MySQL's CHAR_LENGTH() of a binary string count bytes.
    - CAST(X AS <a type of INTEGER affinity>) (lang_expr.html#castexpr): SQLite's INTEGER is 64 bits
      whatever the type's name ("INTEGER ... stored in 0, 1, 2, 3, 4, 6, or 8 bytes", datatype3.html),
      where sqlglot writes INT, 32 bits on each engine: the type is given as BIGINT. "When casting a
      REAL value into an INTEGER, the result is the integer between the REAL value and zero that is
      closest to the REAL value", where DuckDB's, PostgreSQL's and MySQL's casts round (DuckDB 1.4.2
      gives CAST(3.9 AS INT) 4, SQLite 3): a REAL X (storage_classes()) is given trunc(X) first.
    - typeof(X): "a string that indicates the datatype of the expression X: "null", "integer",
      "real", "text", or "blob"" (lang_corefunc.html#typeof), where DuckDB's and PostgreSQL's
      typeof name the engine's type ('INTEGER', 'DECIMAL(2,1)', 'VARCHAR') and ClickHouse's
      toTypeName its own: for an X whose storage class storage_classes() knows, the class's name,
      'null' where X is NULL (a literal's name alone). Another X is left as it is.
    - quote(X): "the text of an SQL literal which is the value of its argument suitable for
      inclusion into an SQL statement. Strings are surrounded by single-quotes with escapes on
      interior quotes as needed. BLOBs are encoded as hexadecimal literals" (lang_corefunc.html#quote),
      NULL as NULL; no other engine has it. For an X of a class storage_classes() knows: an
      INTEGER's text, a TEXT's with each ' doubled between quotes, a BLOB's X'..' of its upper-case
      hex (PostgreSQL's upper(encode(X, 'hex'))), each 'NULL' where X is NULL. A REAL's text is
      SQLite's own (%!.15g, more digits where that doesn't read back: quote(1.0/3) is
      '3.333333333333333148e-01'), and another X: left as they are."""
    from sqlglot import exp

    classes = storage_classes(expression, context.tables)

    def same(node, said):
        nonlocal expression
        if node is expression:
            expression = said
        else:
            node.replace(said)

    for node in [n for n in reversed(list(expression.dfs()))]:
        if isinstance(node, exp.Anonymous) and node.name.upper() in ("LIKELY", "UNLIKELY", "LIKELIHOOD"):
            args = node.expressions
            if node.name.upper() == "LIKELIHOOD":
                y = args[1] if len(args) == 2 else None
                if not (isinstance(y, exp.Literal) and not y.is_string and not y.this.isdigit() and 0.0 <= float(y.this) <= 1.0):
                    continue
            elif len(args) != 1:
                continue
            same(node, exp.Paren(this=args[0].copy()))
        elif isinstance(node, (exp.Upper, exp.Lower)) and dialect in ("duckdb", "postgres"):
            frm, to = (_ASCII_LOWER, _ASCII_UPPER) if isinstance(node, exp.Upper) else (_ASCII_UPPER, _ASCII_LOWER)
            same(node, exp.Anonymous(this="translate", expressions=[node.this.copy(), exp.Literal.string(frm), exp.Literal.string(to)]))
        elif isinstance(node, exp.Round) and node.args.get("decimals") is not None:
            y = node.args["decimals"]
            if isinstance(y, exp.Neg) and isinstance(y.this, exp.Literal) and not y.this.is_string:
                node.set("decimals", exp.Literal.number(0))
            elif not (isinstance(y, exp.Literal) and not y.is_string):
                if any(isinstance(n, exp.Rand) for n in y.walk()):
                    raise NotExpressible(f"not expressible in {dialect}: round() to random() digits, which SQLite's negative-digits rule (lang_corefunc.html#round) would have to evaluate twice")
                node.set("decimals", exp.Case(ifs=[exp.If(this=exp.LT(this=y.copy(), expression=exp.Literal.number(0)), true=exp.Literal.number(0))], default=y.copy()))
        elif isinstance(node, exp.Chr) and dialect == "duckdb" and node.expressions:
            parts = [exp.Chr(expressions=[exp.Coalesce(this=a.copy(), expressions=[exp.Literal.number(0)])]) for a in node.expressions]
            said = parts[0]
            for more in parts[1:]:
                said = exp.DPipe(this=said, expression=more)
            same(node, exp.Paren(this=said) if len(parts) > 1 else said)
        elif isinstance(node, exp.Format):
            same(node, exp.Anonymous(this="printf", expressions=[a.copy() for a in [node.this] + list(node.expressions)]))
        elif isinstance(node, exp.Hex) and dialect in ("duckdb", "mysql", "clickhouse"):
            arg = node.this
            if classes.get(id(arg)) == "INTEGER":
                arg = exp.cast(arg.copy(), exp.DataType.build("TEXT"))
            elif classes.get(id(arg)) == "REAL":
                arg = _real_text(arg, dialect, "hex")
            else:
                arg = arg.copy()
            same(node, exp.Coalesce(this=exp.Hex(this=arg), expressions=[exp.Literal.string("")]))
        elif isinstance(node, exp.Anonymous) and node.name.upper() == "ZEROBLOB" and len(node.expressions) == 1 and dialect in ("duckdb", "postgres"):
            n_ = node.expressions[0]
            if not (isinstance(n_, exp.Null) or classes.get(id(n_)) == "INTEGER"):
                continue
            count = exp.Coalesce(this=n_.copy(), expressions=[exp.Literal.number(0)])
            if dialect == "duckdb":
                said = exp.Anonymous(this="repeat", expressions=[exp.Unhex(this=exp.Literal.string("00")), count])
            else:
                said = exp.Anonymous(this="decode", expressions=[exp.Anonymous(this="repeat", expressions=[exp.Literal.string("00"), count]), exp.Literal.string("hex")])
            same(node, said)
        elif isinstance(node, exp.Length) and classes.get(id(node.this)) == "REAL":
            node.set("this", _real_text(node.this, dialect, "length"))
        elif isinstance(node, exp.Length) and dialect in ("duckdb", "postgres", "clickhouse"):
            arg = classes.get(id(node.this))
            if arg == "INTEGER":
                node.set("this", exp.cast(node.this.copy(), exp.DataType.build("TEXT")))
            elif arg == "BLOB" and dialect != "postgres":
                same(node, exp.Anonymous(this="octet_length" if dialect == "duckdb" else "length", expressions=[node.this.copy()]))
        elif isinstance(node, exp.Typeof) and classes.get(id(node.this), UNKNOWN) != UNKNOWN:
            c, x = classes[id(node.this)], node.this
            if c is None or isinstance(x, exp.Literal) or (isinstance(x, exp.Neg) and isinstance(x.this, exp.Literal)):
                same(node, exp.Literal.string((c or "null").lower()))
            else:
                same(node, exp.Case(ifs=[exp.If(this=exp.Is(this=x.copy(), expression=exp.Null()), true=exp.Literal.string("null"))], default=exp.Literal.string(c.lower())))
        elif isinstance(node, exp.Anonymous) and node.name.upper() == "QUOTE" and len(node.expressions) == 1 and classes.get(id(node.expressions[0]), UNKNOWN) in (None, "INTEGER", "TEXT", "BLOB"):
            x = node.expressions[0]
            c = classes[id(x)]
            if c is None:
                said = exp.Literal.string("NULL")
            elif c == "INTEGER":
                said = exp.cast(x.copy(), exp.DataType.build("TEXT"))
            elif c == "TEXT":
                q = exp.Literal.string("'")
                said = exp.DPipe(this=exp.DPipe(this=q, expression=exp.Anonymous(this="replace", expressions=[x.copy(), q.copy(), exp.Literal.string("''")])), expression=q.copy())
            else:
                hexed = exp.Upper(this=exp.Anonymous(this="encode", expressions=[x.copy(), exp.Literal.string("hex")])) if dialect == "postgres" else exp.Hex(this=x.copy())
                said = exp.DPipe(this=exp.DPipe(this=exp.Literal.string("X'"), expression=hexed), expression=exp.Literal.string("'"))
            same(node, said if c is None else exp.Coalesce(this=said, expressions=[exp.Literal.string("NULL")]))
        elif type(node) is exp.Cast and isinstance(node.to, exp.DataType) and affinity(node.to.sql(dialect="sqlite")) == "INTEGER":
            if classes.get(id(node.this)) == "REAL":
                node.set("this", exp.Trunc(this=node.this.copy()))
            node.set("to", exp.DataType.build("BIGINT"))
    return expression


def extremes(expression, dialect):
    """`expression` with each multi-argument max() and min() said as the dialect's GREATEST and LEAST
    with SQLite's NULL. SQLite's "Built-In Scalar SQL Functions" (https://www.sqlite.org/lang_corefunc.html#max_scalar):
    "The multi-argument max() function returns the argument with the maximum value, or return NULL
    if any argument is NULL" (min() the same, lang_corefunc.html#min_scalar). PostgreSQL's GREATEST
    and LEAST ignore NULLs ("NULL values in the argument list are ignored. The result will be NULL
    only if all the expressions evaluate to NULL", its manual, 9.18.4), as DuckDB's and ClickHouse's
    (24.12 on) do; MySQL's return NULL "if any argument is NULL" (MySQL 8.4 manual, 14.4.2). So each
    is said as CASE WHEN an argument IS NULL THEN NULL ELSE GREATEST(...) END, exact on every engine
    whichever way its GREATEST takes NULLs. That names each argument twice, which is the same only
    when an argument gives the same value each time: one that calls random() or randomblob()
    (lang_corefunc.html: "a pseudo-random integer", "random bytes") is not expressible. sqlglot
    otherwise writes PostgreSQL's GREATEST unguarded, and DuckDB's and ClickHouse's max() of
    several arguments, which is their aggregate's top-N or an error."""
    from sqlglot import exp

    # Innermost first, so one inside another's argument is said before that argument is copied.
    for node in [n for n in reversed(list(expression.dfs())) if isinstance(n, (exp.Max, exp.Min)) and n.expressions]:
        args = [node.this] + list(node.expressions)
        if any(isinstance(n, exp.Rand) or (isinstance(n, exp.Anonymous) and n.name.upper() == "RANDOMBLOB") for a in args for n in a.walk()):
            raise NotExpressible(f"not expressible in {dialect}: a multi-argument {node.key}() of a random() or randomblob() argument (lang_corefunc.html), which SQLite's NULL rule would have to evaluate twice")
        pick = (exp.Greatest if isinstance(node, exp.Max) else exp.Least)(this=args[0].copy(), expressions=[a.copy() for a in args[1:]], ignore_nulls=True)
        nulls = [exp.Is(this=a.copy(), expression=exp.Null()) for a in args]
        said = exp.Case(ifs=[exp.If(this=exp.or_(*nulls), true=exp.Null())], default=pick)
        if node is expression:
            expression = said
        else:
            node.replace(said)
    return expression


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
            e = spoken(globbed(functions_said(extremes(keyed(typed(liked(divided(e, context or Context(), dialect), context or Context(), dialect), context, dialect), dialect), dialect), context or Context(), dialect), dialect), dialect)
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


def _names(e):
    """The tables and views statement `e` names (upper-cased), less the CTEs it defines, and the one
    a CREATE TABLE or CREATE VIEW of it creates (None for another statement)."""
    from sqlglot import exp

    ctes = {c.alias_or_name.upper() for c in e.find_all(exp.CTE)}
    made = None
    if isinstance(e, exp.Create) and e.args.get("kind") in ("TABLE", "VIEW"):
        target = e.this.this if isinstance(e.this, exp.Schema) else e.this
        made = target.name.upper() if isinstance(target, exp.Table) else None
    named = {t.name.upper() for t in e.find_all(exp.Table) if isinstance(t.this, exp.Identifier)} - ctes
    return named - {made}, made


def records_in(records, dialect, around=None):
    """Each of a file's records in `dialect` (record_in()), in order, where a record that names a
    table or view whose CREATE the dialect couldn't say (one SQLite ran: not a `statement error`)
    is itself untranslatable, with that CREATE's line and reason: there the table isn't made, so
    the record would fail on the engine for the translation's sake, not the engine's (G2 step 4:
    an engine is never charged with a statement the corpus couldn't give it). A
    translated CREATE of the same name (after a DROP) makes it again; a DROP of it is
    untranslatable too, and forgets it. `around` is the file's contexts() (computed when None)."""
    import sqlglot
    from sqlglot import exp

    around = around if around is not None else contexts(records)
    missing, out = {}, []
    for r, c in zip(records, around):
        sql, why = record_in(r, dialect, c)
        try:
            statements = [e for e in sqlglot.parse(comment_closed(defaults_dropped(r.sql)), read=reader()) if e]
        except Exception:  # sqlglot can't read it: record_in() has said so
            statements = []
        ran = not (r.kind == "statement" and r.expect == "error")
        named, made = set(), []
        for e in statements:
            n, m = _names(e)
            named |= n
            made.append((m, e))
        gone = sorted(n for n in named if n in missing)
        if why is None and gone:
            line, reason = missing[gone[0]]
            why = f"untranslatable in {dialect}: names {gone[0].lower()}, whose CREATE (line {line}) is untranslatable there: {reason}"
            sql = None
        for m, e in made:
            if m is None or not ran:
                continue
            if why is None:
                missing.pop(m, None)
            elif not (e.args.get("exists") and m in c.tables and m not in missing):
                missing[m] = (r.line, why)
        if ran:
            for e in statements:
                if isinstance(e, exp.Drop) and e.args.get("kind") in ("TABLE", "VIEW") and isinstance(e.this, exp.Table):
                    missing.pop(e.this.name.upper(), None)
        out.append((sql, why))
    return out


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
                for r, (sql, why) in zip(records, records_in(records, d, around)):
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
