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

        def _parse_statement(self):
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
    Every other function (sum, total, avg over values of any type, min, max, first_value, ...) has no
    exact arithmetic: not expressible."""
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
    if not isinstance(func, exp.Count) or isinstance(func.this, exp.Distinct) or not holds:
        raise NotExpressible(why + "; only count() over a frame holding the current row has an exact rewrite)")
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


def typed(expression):
    """`expression` with each column declared without a type given the type SQLite's affinity rules
    give it, so engines that require a column's type (DuckDB, PostgreSQL, MySQL, ClickHouse) take the
    table. SQLite's "Datatypes In SQLite", section 3.1 "Determination Of Column Affinity", rule 3:
    "If the declared type for a column contains the string "BLOB" or if no type is specified then
    the column has affinity BLOB" (https://www.sqlite.org/datatype3.html#determination_of_column_affinity);
    section 3: "A column with affinity BLOB does not prefer one storage class over another and no
    attempt is made to coerce data from one storage class into another." So an untyped column is
    declared BLOB, as SQLite's own declared BLOB reads, which sqlglot writes as each dialect's
    binary type (DuckDB's BLOB, PostgreSQL's BYTEA, MySQL's BLOB, ClickHouse's String: a column
    declared BLOB in the corpus is written the same). A value of another storage class written to it is
    then each engine's to take or refuse, as a BLOB column's would be: no type is picked here.

    A STRICT table ("STRICT Tables", https://www.sqlite.org/stricttables.html) is one whose columns
    hold only their declared type: "SQLite attempts to coerce the data into the appropriate type ...
    If the value cannot be losslessly converted in the specified datatype, then an
    SQLITE_CONSTRAINT_DATATYPE error is raised", which is how every column of the four engines holds
    its type, so the table is declared without STRICT (DuckDB has no such word, and PostgreSQL, MySQL
    and ClickHouse refuse it); how each engine converts a value is its type system's, which the
    oracle classifies as such. Its ANY columns take any value unchanged ("no type coercion"), as a
    BLOB column does: declared BLOB, as above."""
    from sqlglot import exp

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
                        column.set("kind", exp.DataType(this=exp.DataType.Type.BLOB))
    for column in expression.find_all(exp.ColumnDef):
        if column.args.get("kind") is None:
            column.set("kind", exp.DataType(this=exp.DataType.Type.BLOB))
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
    if dialect in ("mysql", "clickhouse"):
        for window in list(expression.find_all(exp.Window)):
            spec = window.args.get("spec")
            if isinstance(spec, exp.WindowSpec) and spec.args.get("exclude") is not None:
                said = excluded(window, dialect)
                if said is not window:
                    window.replace(said)
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


def translate(sql, dialect):
    """`sql`, in SQLite's dialect, in `dialect`: (text, None), or (None, why) when sqlglot can't say
    it there or the dialect has nothing that means the same. The one translation the corpus's tools
    share (probe.py's too)."""
    if dialect == "sqlite":
        return sql, None
    import sqlglot

    text = comment_closed(defaults_dropped(sql))
    clause = conflict_clause(text)
    if clause is not None:
        return None, f"not expressible in {dialect}: SQLite's conflict resolution {clause} (https://www.sqlite.org/lang_conflict.html) has no equivalent there"
    try:
        # sqlglot.transpile's steps, each statement given its untyped columns' types between them.
        out = [spoken(typed(e), dialect).sql(dialect=dialect, unsupported_level=sqlglot.ErrorLevel.RAISE) if e else "" for e in sqlglot.parse(text, read=reader())]
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


def record_in(record, dialect):
    """`record` (slt.Record) in `dialect`, as translate() says it. A record SQLite refuses
    (`statement error`) that sqlglot can't read either is given as SQLite holds it, its default
    clauses left out: what the record asks of an engine is that it refuse the statement too, and an
    engine that can't read it refuses it (the corpus's records, as sqllogictest's, don't say which
    error). One sqlglot can read is translated, so an engine that refuses it does so for the
    statement's sake, not its spelling."""
    sql, why = translate(record.sql, dialect)
    if why is not None and record.kind == "statement" and record.expect == "error":
        return defaults_dropped(record.sql), None
    return sql, why


def files(pool):
    for root, _dirs, names in os.walk(pool, followlinks=True):
        if os.path.relpath(root, pool).split(os.sep)[0] == "translations":
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
    for rel in files(pool):
        records = [r for kind, r in slt.parse(os.path.join(pool, rel)) if kind == "record"]
        for d in dialects:
            out = os.path.join(pool, "translations", d, rel + ".jsonl")
            os.makedirs(os.path.dirname(out), exist_ok=True)
            with open(out, "w", encoding="utf-8") as f:
                for r in records:
                    sql, why = record_in(r, d)
                    counts[d][why is not None] += 1
                    f.write(json.dumps({"line": r.line, "sql": sql} if why is None else {"line": r.line, "untranslatable": why}) + "\n")
    for d, (ok, no) in counts.items():
        print(f"{d}: {ok} records translated, {no} untranslatable")


if __name__ == "__main__":
    main()
