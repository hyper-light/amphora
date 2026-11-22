#!/usr/bin/env python3
"""Each corpus record translated once per dialect (G2 step 4), at corpus build time, by sqlglot (the
translator sources.json pins), so the differential harness (crates/corpus, amphora_corpus::dialect)
runs every engine on SQL it speaks and never translates as it runs.

For every `.test` file under the pool (symbolic links followed: the pool links sqllogictest's
checkout), each record's SQL, in SQLite's dialect as the corpus holds it, is written in each dialect
to <pool>/translations/<dialect>/<file>.jsonl, a JSON line a record in file order:
  {"line": n, "sql": "..."}              its text in the dialect
  {"line": n, "untranslatable": "..."}   sqlglot can't say it there, and why
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
    then each engine's to take or refuse, as a BLOB column's would be: no type is picked here."""
    from sqlglot import exp

    # sqlglot reads a column with neither type nor constraint as a bare name in CREATE TABLE's
    # schema, one with constraints only as a ColumnDef without a kind.
    for create in expression.find_all(exp.Create):
        schema = create.this
        if create.args.get("kind") == "TABLE" and isinstance(schema, exp.Schema):
            for column in list(schema.expressions):
                if isinstance(column, exp.Identifier):
                    column.replace(exp.ColumnDef(this=column.copy()))
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
    - a window frame's EXCLUDE NO OTHERS, its default, is left out."""
    from sqlglot import exp

    # A window frame's `EXCLUDE NO OTHERS` is its default (SQLite's "Window Functions", 2.2.4 "The
    # EXCLUDE clause": "the default is EXCLUDE NO OTHERS", as in standard SQL), which MySQL and
    # ClickHouse, that have no EXCLUDE, do unasked: it is left out. The other three exclusions
    # have no spelling there, and sqlglot says so.
    for frame in expression.find_all(exp.WindowSpec):
        exclude = frame.args.get("exclude")
        if isinstance(exclude, exp.Var) and " ".join(exclude.name.upper().split()) == "NO OTHERS":
            frame.set("exclude", None)
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
    it there. The one translation the corpus's tools share (probe.py's too)."""
    if dialect == "sqlite":
        return sql, None
    import sqlglot

    try:
        # sqlglot.transpile's steps, each statement given its untyped columns' types between them.
        out = [
            spoken(typed(e), dialect).sql(dialect=dialect, unsupported_level=sqlglot.ErrorLevel.RAISE) if e else ""
            for e in sqlglot.parse(defaults_dropped(sql), read="sqlite")
        ]
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
