#!/usr/bin/env python3
"""Hand-written spellings (G2 step 1): how each other engine says a probe's feature where sqlglot
can't translate it, or translates it into something the engine refuses though the engine has the
feature, and why an engine has no way to say it at all. probe.py takes a probe's rule for the
engine's dialect over sqlglot's translation; matrix.py shows which spelling each answer came from.

A rule, keyed by probe id and then dialect (postgres also speaks for OrioleDB and CedarDB):
  {"query": ..., "setup_tail": [...]}   the probe in that dialect; `setup_tail` replaces the probe's
                                         own setup after the shared tables t and u (sqlglot still
                                         translates those), `setup` the whole of it
  "none: <why>"                          the dialect has no way to say it

usage: python3 spellings.py <out spellings.json>
"""
import json
import sys

PG, MY, DUCK, CH = "postgres", "mysql", "duckdb", "clickhouse"
RULES = {}


def rule(pid, **by_dialect):
    for d, r in by_dialect.items():
        RULES.setdefault(pid, {})[{"pg": PG, "my": MY, "duck": DUCK, "ch": CH}[d]] = r


def none(why):
    return f"none: {why}"


# ---------------------------------------------------------------- statements and clauses only SQLite has
PER_STATEMENT = "resolves conflicts per statement ({}), not per constraint"
rule("syntax:column-constraint#1",
     pg=none("PostgreSQL " + PER_STATEMENT.format("INSERT ... ON CONFLICT") + ", and a primary key has no direction"),
     my=none("MySQL " + PER_STATEMENT.format("REPLACE, INSERT ... ON DUPLICATE KEY UPDATE")),
     duck=none("DuckDB " + PER_STATEMENT.format("INSERT OR REPLACE, ON CONFLICT")),
     ch=none("ClickHouse keeps no unique or primary-key constraint to resolve: its sorting key accepts duplicates"))
rule("syntax:conflict-clause",
     pg=none("PostgreSQL " + PER_STATEMENT.format("INSERT ... ON CONFLICT DO NOTHING")),
     my=none("MySQL " + PER_STATEMENT.format("INSERT IGNORE")),
     duck=none("DuckDB " + PER_STATEMENT.format("INSERT OR IGNORE")),
     ch=none("ClickHouse keeps no unique constraint to resolve"))
STRICT_WHY = "column types are always enforced there; it has no ANY type and no rowid to leave out"
rule("syntax:table-options#2", pg=none("PostgreSQL's " + STRICT_WHY), my=none("MySQL's " + STRICT_WHY), duck=none("DuckDB's " + STRICT_WHY), ch=none("ClickHouse's " + STRICT_WHY))
rule("syntax:table-options#3", pg=none("PostgreSQL's " + STRICT_WHY), my=none("MySQL's " + STRICT_WHY), duck=none("DuckDB's " + STRICT_WHY), ch=none("ClickHouse's " + STRICT_WHY))
rule("word:STRICT#1", pg=none("PostgreSQL's " + STRICT_WHY), my=none("MySQL's " + STRICT_WHY), duck=none("DuckDB's " + STRICT_WHY), ch=none("ClickHouse's " + STRICT_WHY))
rule("word:STRICT#2", pg=none("PostgreSQL's " + STRICT_WHY), my=none("MySQL's " + STRICT_WHY), duck=none("DuckDB's " + STRICT_WHY), ch=none("ClickHouse's " + STRICT_WHY))
ROWID_WHY = "tables there have no rowid: a primary key table is already clustered by it (InnoDB) or keeps rows apart from its indexes"
rule("syntax:table-options#1", pg=none("PostgreSQL's " + ROWID_WHY), my=none("MySQL's " + ROWID_WHY), duck=none("DuckDB's " + ROWID_WHY), ch=none("ClickHouse's " + ROWID_WHY))
rule("word:ROWID", pg=none("PostgreSQL's " + ROWID_WHY), my=none("MySQL's " + ROWID_WHY), duck=none("DuckDB's " + ROWID_WHY), ch=none("ClickHouse's " + ROWID_WHY))
rule("syntax:update-stmt#2",
     my={"query": "UPDATE IGNORE t SET a = a WHERE a = 1"},
     pg=none("PostgreSQL's UPDATE has no conflict resolution: a violated constraint fails the statement"),
     duck=none("DuckDB's UPDATE has no conflict resolution"),
     ch=none("ClickHouse changes rows with ALTER TABLE ... UPDATE mutations, which have no conflict resolution"))
rule("syntax:insert-stmt#2",
     pg={"query": "INSERT INTO pk VALUES (1, 'dup') ON CONFLICT DO NOTHING"}, my={"query": "INSERT IGNORE INTO pk VALUES (1, 'dup')"},
     ch=none("ClickHouse keeps no unique constraint, so nothing conflicts"))
rule("syntax:insert-stmt#3",
     pg={"query": "INSERT INTO pk VALUES (1, 'new') ON CONFLICT (k) DO UPDATE SET v = excluded.v"}, my={"query": "REPLACE INTO pk VALUES (1, 'new')"},
     ch=none("ClickHouse keeps no unique constraint; a ReplacingMergeTree merges duplicates later, not at the insert"))
rule("syntax:insert-stmt#6",
     pg={"query": "INSERT INTO pk VALUES (2, 'two') ON CONFLICT (k) DO UPDATE SET v = excluded.v"}, duck={"query": "INSERT OR REPLACE INTO pk VALUES (2, 'two')"},
     ch=none("ClickHouse keeps no unique constraint; a ReplacingMergeTree merges duplicates later, not at the insert"))
rule("syntax:qualified-table-name",
     pg=none("PostgreSQL takes no index hints: its planner alone chooses (pg_hint_plan is an extension)"),
     my={"query": "SELECT a FROM t FORCE INDEX (ti) WHERE a > 3 ORDER BY a"},
     duck=none("DuckDB takes no index hints"),
     ch=none("ClickHouse takes no index hints; its primary key and skipping indexes are used by the planner"))
rule("syntax:release-stmt",
     pg={"setup_tail": ["BEGIN", "SAVEPOINT sp1"], "query": "RELEASE SAVEPOINT sp1"},
     my={"setup_tail": ["BEGIN", "SAVEPOINT sp1"], "query": "RELEASE SAVEPOINT sp1"},
     duck=none("DuckDB has no savepoints"), ch=none("ClickHouse has no savepoints"))
rule("syntax:savepoint-stmt",
     pg={"setup_tail": ["BEGIN"], "query": "SAVEPOINT sp1"}, my={"setup_tail": ["BEGIN"], "query": "SAVEPOINT sp1"},
     duck=none("DuckDB has no savepoints"), ch=none("ClickHouse has no savepoints"))
rule("syntax:rollback-stmt",
     pg={"setup_tail": ["BEGIN", "SAVEPOINT sp1"], "query": "ROLLBACK TO SAVEPOINT sp1"}, my={"setup_tail": ["BEGIN", "SAVEPOINT sp1"], "query": "ROLLBACK TO SAVEPOINT sp1"},
     duck=none("DuckDB has no savepoints"), ch=none("ClickHouse has no savepoints"))
VTAB_WHY = "SQLite's virtual-table modules (FTS5, R-Tree) are its own; {} is the counterpart, with its own syntax"
rule("syntax:create-virtual-table-stmt#1",
     pg=none(VTAB_WHY.format("text search over tsvector with a GIN index")), my=none(VTAB_WHY.format("a FULLTEXT index")),
     duck=none(VTAB_WHY.format("the fts extension's PRAGMA create_fts_index")), ch=none(VTAB_WHY.format("a full-text (inverted) index")))
rule("syntax:create-virtual-table-stmt#2",
     pg=none(VTAB_WHY.format("a GiST index on geometric types")), my=none(VTAB_WHY.format("a SPATIAL index")),
     duck=none(VTAB_WHY.format("the spatial extension's R-tree index")), ch=none(VTAB_WHY.format("nothing built in")))
ATTACH_WHY = "{} reaches other databases another way ({})"
rule("syntax:attach-stmt", pg=none(ATTACH_WHY.format("PostgreSQL", "dblink or postgres_fdw")), my=none(ATTACH_WHY.format("MySQL", "every database of the server is open to a session")),
     ch=none(ATTACH_WHY.format("ClickHouse", "databases are the server's; ATTACH there re-registers a detached table")))
rule("syntax:detach-stmt", pg=none(ATTACH_WHY.format("PostgreSQL", "dblink or postgres_fdw")), my=none(ATTACH_WHY.format("MySQL", "every database of the server is open to a session")),
     ch=none(ATTACH_WHY.format("ClickHouse", "databases are the server's; DETACH there unregisters a table")))
rule("syntax:begin-stmt#1", pg={"query": "BEGIN"}, my={"query": "START TRANSACTION"}, duck={"query": "BEGIN TRANSACTION"}, ch=none("ClickHouse's transactions are experimental and off by default"))
rule("syntax:begin-stmt#2", pg={"query": "BEGIN ISOLATION LEVEL SERIALIZABLE"}, my={"query": "START TRANSACTION"}, duck={"query": "BEGIN TRANSACTION"}, ch=none("ClickHouse's transactions are experimental and off by default"))
rule("syntax:begin-stmt#3", pg={"query": "BEGIN ISOLATION LEVEL SERIALIZABLE"}, my={"query": "START TRANSACTION"}, duck={"query": "BEGIN TRANSACTION"}, ch=none("ClickHouse's transactions are experimental and off by default"))
rule("syntax:vacuum-stmt", my={"query": "OPTIMIZE TABLE t"}, ch={"query": "OPTIMIZE TABLE t FINAL"})
rule("syntax:analyze-stmt#1", my={"query": "ANALYZE TABLE t, u"}, ch=none("ClickHouse gathers no planner statistics on request"))
rule("syntax:analyze-stmt#2", my={"query": "ANALYZE TABLE t"}, ch=none("ClickHouse gathers no planner statistics on request"))
rule("syntax:reindex-stmt#1", my=none("MySQL rebuilds indexes with ALTER TABLE ... FORCE or OPTIMIZE TABLE, per table"), duck=none("DuckDB has no REINDEX"),
     ch=none("ClickHouse rebuilds a skipping index with ALTER TABLE ... MATERIALIZE INDEX, per index"))
rule("syntax:reindex-stmt#2", pg={"query": "REINDEX TABLE t"}, my={"query": "ALTER TABLE t FORCE"}, duck=none("DuckDB has no REINDEX"),
     ch=none("ClickHouse rebuilds a skipping index with ALTER TABLE ... MATERIALIZE INDEX, per index"))
rule("word:EXPRESSIONS", pg=none("PostgreSQL reindexes by index, table, schema or database, not by kind"), my=none("MySQL has no REINDEX"),
     duck=none("DuckDB has no REINDEX"), ch=none("ClickHouse has no REINDEX"))
rule("syntax:sql-stmt#2", pg={"query": "EXPLAIN SELECT a FROM t WHERE a = 1"}, my={"query": "EXPLAIN FORMAT=TREE SELECT a FROM t WHERE a = 1"},
     duck={"query": "EXPLAIN SELECT a FROM t WHERE a = 1"}, ch={"query": "EXPLAIN PLAN SELECT a FROM t WHERE a = 1"})
TRIGGER_WHY = "PostgreSQL's triggers run a function written apart (CREATE FUNCTION ... RETURNS trigger, then CREATE TRIGGER ... EXECUTE FUNCTION)"
PG_TRIGGER = ["CREATE FUNCTION tr_f() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.a > 100 THEN INSERT INTO u VALUES (NEW.a, 'trig'); END IF; RETURN NEW; END $$"]
rule("syntax:create-trigger-stmt#1",
     pg={"setup_tail": ["CREATE VIEW v1 AS SELECT a, b FROM t"] + PG_TRIGGER, "query": "CREATE TRIGGER tr AFTER INSERT ON t FOR EACH ROW EXECUTE FUNCTION tr_f()"},
     my={"setup_tail": ["CREATE VIEW v1 AS SELECT a, b FROM t"], "query": "CREATE TRIGGER tr AFTER INSERT ON t FOR EACH ROW BEGIN IF NEW.a > 100 THEN INSERT INTO u VALUES (NEW.a, 'trig'); END IF; END"},
     duck=none("DuckDB has no triggers"), ch=none("ClickHouse has no triggers (a materialized view acts on inserts)"))
for n in (2, 3, 4):
    rule(f"syntax:create-trigger-stmt#{n}", pg=none(TRIGGER_WHY + "; this form's RAISE/OF/INSTEAD OF clauses are spelled inside that function"),
         duck=none("DuckDB has no triggers"), ch=none("ClickHouse has no triggers (a materialized view acts on inserts)"))
rule("syntax:raise-function", pg=none(TRIGGER_WHY + "; RAISE(ABORT, ...) is RAISE EXCEPTION there"), my=none("MySQL signals errors with SIGNAL SQLSTATE in a trigger body"),
     duck=none("DuckDB has no triggers"), ch=none("ClickHouse has no triggers"))
rule("syntax:column-def#1", duck={"setup_tail": [], "query": "CREATE SEQUENCE cd_a; CREATE TABLE cd(a INTEGER PRIMARY KEY DEFAULT nextval('cd_a'), v TEXT); INSERT INTO cd(v) VALUES ('x'), ('y'); SELECT a, v FROM cd ORDER BY a"},
     ch=none("ClickHouse has no auto-incrementing column (generateSerialID is a separate function)"))
rule("syntax:column-def#2", pg=none("PostgreSQL has no case-insensitive collation built in (a nondeterministic ICU collation is created by hand, or citext)"),
     my={"query": "CREATE TABLE cd(b VARCHAR(10) COLLATE utf8mb4_0900_ai_ci); INSERT INTO cd VALUES ('Abc'); SELECT count(*) FROM cd WHERE b = 'aBC'"},
     ch=none("ClickHouse compares strings by bytes; lower() is its case-insensitive comparison"))
rule("syntax:column-def#4", pg=none("PostgreSQL 17 stores generated columns only (VIRTUAL arrived in 18)"),
     ch={"query": "CREATE TABLE cd(a Int32, d Int32 ALIAS a + 1) ENGINE = MergeTree ORDER BY a; INSERT INTO cd(a) VALUES (4); SELECT d FROM cd"})
rule("syntax:type-name#2", pg={"query": "CREATE TABLE tn(d NUMERIC(20, 0)); INSERT INTO tn VALUES (7); SELECT d FROM tn"},
     my={"query": "CREATE TABLE tn(d BIGINT UNSIGNED); INSERT INTO tn VALUES (7); SELECT d FROM tn"},
     duck={"query": "CREATE TABLE tn(d UBIGINT); INSERT INTO tn VALUES (7); SELECT d FROM tn"},
     ch={"query": "CREATE TABLE tn(d UInt64) ENGINE = MergeTree ORDER BY d; INSERT INTO tn VALUES (7); SELECT d FROM tn"})

# ---------------------------------------------------------------- operators
rule("operator:GLOB", pg={"query": "SELECT 'abc' ~ '^a.c$'"}, my={"query": "SELECT REGEXP_LIKE('abc', '^a.c$', 'c')"}, ch={"query": "SELECT match('abc', '^a.c$')"})
rule("operator:NOT GLOB", pg={"query": "SELECT 'abc' !~ '^A'"}, my={"query": "SELECT NOT REGEXP_LIKE('abc', '^A', 'c')"}, ch={"query": "SELECT NOT match('abc', '^A')"})
rule("operator:REGEXP", pg={"query": "SELECT 'abc' ~ 'b', 'abc' ~ '^b'"}, duck={"query": "SELECT regexp_matches('abc', 'b'), regexp_matches('abc', '^b')"},
     ch={"query": "SELECT match('abc', 'b'), match('abc', '^b')"})
rule("operator:NOT REGEXP", pg={"query": "SELECT 'abc' !~ 'z'"}, duck={"query": "SELECT NOT regexp_matches('abc', 'z')"}, ch={"query": "SELECT NOT match('abc', 'z')"})
rule("operator:MATCH",
     pg={"setup": ["CREATE TABLE ft(body TEXT)", "INSERT INTO ft VALUES ('hello world'), ('goodbye')"], "query": "SELECT body FROM ft WHERE to_tsvector('simple', body) @@ to_tsquery('simple', 'hello')"},
     my={"setup": ["CREATE TABLE ft(body TEXT, FULLTEXT (body)) ENGINE = InnoDB", "INSERT INTO ft VALUES ('hello world'), ('goodbye')"], "query": "SELECT body FROM ft WHERE MATCH(body) AGAINST ('hello')"},
     duck=none("DuckDB's full-text search is the fts extension (PRAGMA create_fts_index), installed apart"),
     ch={"setup": ["CREATE TABLE ft(body String) ENGINE = MergeTree ORDER BY tuple()", "INSERT INTO ft VALUES ('hello world'), ('goodbye')"], "query": "SELECT body FROM ft WHERE hasToken(body, 'hello')"})
rule("operator:NOT MATCH", pg=none("SQLite's NOT MATCH needs an application-defined match(); PostgreSQL negates a text search with NOT (... @@ ...)"),
     my=none("SQLite's NOT MATCH needs an application-defined match(); MySQL negates NOT MATCH(...) AGAINST (...)"),
     duck=none("SQLite's NOT MATCH needs an application-defined match()"), ch=none("SQLite's NOT MATCH needs an application-defined match()"))

# ---------------------------------------------------------------- functions with another name or form
rule("function.core:instr", pg={"query": "SELECT strpos('hello', 'll')"}, ch={"query": "SELECT position('hello', 'll')"})
rule("function.core:char", pg={"query": "SELECT chr(72) || chr(105)"}, ch={"query": "SELECT char(72, 105)"})
rule("function.core:unicode", pg={"query": "SELECT ascii('A')"}, my={"query": "SELECT ord('A')"}, ch={"query": "SELECT ascii('A')"})
rule("function.core:likely", pg={"query": "SELECT 1 = 1"}, my={"query": "SELECT 1 = 1"}, duck={"query": "SELECT 1 = 1"}, ch={"query": "SELECT 1 = 1"})
rule("function.core:unlikely", pg={"query": "SELECT 1 = 2"}, my={"query": "SELECT 1 = 2"}, duck={"query": "SELECT 1 = 2"}, ch={"query": "SELECT 1 = 2"})
rule("function.core:likelihood", pg={"query": "SELECT 1 = 1"}, my={"query": "SELECT 1 = 1"}, duck={"query": "SELECT 1 = 1"}, ch={"query": "SELECT 1 = 1"})
rule("function.core:changes", pg=none("PostgreSQL reports rows changed to the client (the command tag, GET DIAGNOSTICS in PL/pgSQL)"), my={"query": "SELECT row_count()"},
     duck=none("DuckDB reports rows changed to the client"), ch=none("ClickHouse reports rows written to the client"))
rule("function.core:total_changes", pg=none("PostgreSQL keeps no per-connection count of rows changed"), my=none("MySQL keeps no per-connection count of rows changed"),
     duck=none("DuckDB keeps no per-connection count of rows changed"), ch=none("ClickHouse keeps no per-connection count of rows changed"))
rule("function.core:last_insert_rowid", pg=none("PostgreSQL has no rowid; INSERT ... RETURNING or currval() give a new key"), my={"query": "SELECT last_insert_id() >= 0"},
     duck=none("DuckDB has no rowid counter; INSERT ... RETURNING gives a new key"), ch=none("ClickHouse has no rowid"))
rule("function.core:sqlite_version", pg={"query": "SELECT pg_typeof(version())::text = 'text'"}, my={"query": "SELECT version() IS NOT NULL"},
     duck={"query": "SELECT typeof(version())"}, ch={"query": "SELECT toTypeName(version())"})
rule("function.core:sqlite_source_id", pg=none("PostgreSQL has no source identifier function (version() names the release)"),
     my=none("MySQL has no source identifier function"), duck={"query": "SELECT typeof(source_id) FROM pragma_version()"}, ch=none("ClickHouse has no source identifier function"))
rule("function.core:sqlite_compileoption_get", pg=none("PostgreSQL reports build options with pg_config, outside SQL"), my=none("MySQL has no compile-option query"),
     duck=none("DuckDB has no compile-option query"), ch={"query": "SELECT toTypeName(value) FROM system.build_options LIMIT 1"})
rule("function.core:sqlite_compileoption_used", pg=none("PostgreSQL reports build options with pg_config, outside SQL"), my=none("MySQL has no compile-option query"),
     duck=none("DuckDB has no compile-option query"), ch=none("ClickHouse lists its build options in system.build_options"))
rule("function.core:sqlite_offset", pg=none("PostgreSQL rows have no byte offset in a file (ctid names a row's page and slot)"), my=none("MySQL rows have no byte offset query"),
     duck=none("DuckDB rows have no byte offset query"), ch=none("ClickHouse rows have no byte offset query"))
rule("function.core:load_extension", pg=none("PostgreSQL loads extensions with CREATE EXTENSION, a statement"),
     my=none("MySQL loads plugins with INSTALL PLUGIN, a statement"), duck=none("DuckDB loads extensions with INSTALL and LOAD, statements"), ch=none("ClickHouse has no loadable extensions"))
rule("function.core:soundex", duck=none("DuckDB has no soundex()"), ch=none("ClickHouse has no soundex()"))
rule("function.core:randomblob", pg={"query": "SELECT length(decode(md5(random()::text) || md5(random()::text), 'hex')) >= 8"}, my={"query": "SELECT length(random_bytes(8))"},
     duck=none("DuckDB has no random-bytes function"), ch={"query": "SELECT length(randomString(8))"})
rule("function.core:zeroblob", pg={"query": "SELECT length(decode(repeat('00', 4), 'hex'))"}, my={"query": "SELECT length(repeat(char(0), 4))"},
     duck={"query": "SELECT octet_length(repeat('\\x00'::BLOB, 4))"}, ch={"query": "SELECT length(repeat('\\0', 4))"})
rule("function.core:quote", ch=none("ClickHouse has no function that writes a value as an SQL literal"), pg={"query": "SELECT quote_literal('it''s'), quote_nullable(NULL), 'X''AB'''"})
rule("function.core:length", ch={"query": "SELECT lengthUTF8('héllo'), length(unhex('0102')), lengthUTF8(NULL)"})
rule("function.core:typeof", pg={"query": "SELECT pg_typeof(1)::text, pg_typeof(1.5)::text, pg_typeof('a'::text)::text, pg_typeof('\\x00'::bytea)::text, pg_typeof(NULL)::text"},
     my=none("MySQL has no function that names a value's type"), ch={"query": "SELECT toTypeName(1), toTypeName(1.5), toTypeName('a'), toTypeName(unhex('00')), toTypeName(NULL)"})
rule("storage_class:BLOB", pg={"query": "SELECT pg_typeof('\\x01'::bytea)::text"}, my=none("MySQL has no function that names a value's type"), ch={"query": "SELECT toTypeName(unhex('01'))"})
rule("syntax:literal-value", ch={"query": "SELECT 1, 1.5, 'txt', unhex('0F'), NULL, true, false, toTypeName(today()), toTypeName(now()), toTypeName(now())"})
rule("syntax:numeric-literal", ch={"query": "SELECT 0x1F, 1e3, .5, 1000000, 2.5E-1"})

# ---------------------------------------------------------------- PostgreSQL (and OrioleDB, CedarDB) and DuckDB: functions by their own names
J = "'{\"a\":1,\"b\":[1,2,{\"c\":3}],\"d\":null}'"
JB = J + "::jsonb"
rule("function.core:format", pg={"query": "SELECT format('%s-%s', 7, 'x')"})
rule("function.core:printf", pg={"query": "SELECT lpad(to_char(3.14159, 'FM990.00'), 5) || '|' || rpad('a', 3) || '|' || lpad('7', 3, '0')"})
rule("function.core:glob", pg={"query": "SELECT 'abc' ~ '^a.*$', 'bc' ~ '^a.*$'"}, my={"query": "SELECT REGEXP_LIKE('abc', '^a.*$', 'c'), REGEXP_LIKE('bc', '^a.*$', 'c')"},
     ch={"query": "SELECT match('abc', '^a.*$'), match('bc', '^a.*$')"})
rule("function.core:hex", pg={"query": "SELECT upper(encode('AB'::bytea, 'hex')), upper(encode('255'::bytea, 'hex'))"})
rule("function.core:random", pg={"query": "SELECT pg_typeof(random())::text"}, ch={"query": "SELECT toTypeName(rand64())"})
rule("function.core:soundex", pg=none("PostgreSQL's soundex() is in the fuzzystrmatch extension, not installed by default"))
rule("function.core:unhex", pg={"query": "SELECT decode('4142', 'hex')"})
rule("function.core:unistr_quote", pg=none("PostgreSQL has no function that writes text as a unistr() literal"), my=none("MySQL has no unistr_quote()"),
     duck=none("DuckDB has no unistr_quote()"), ch=none("ClickHouse has no unistr_quote()"))
rule("function.core:unistr", duck=none("DuckDB has no unistr(); chr() builds one character at a time"))
rule("function.core:length", duck={"query": "SELECT length('héllo'), octet_length('\\x01\\x02'::BLOB), length(NULL)"})
rule("function.core:max", duck={"query": "SELECT greatest(1, 5, 3)"})
rule("function.core:min", duck={"query": "SELECT least(4, 2, 8)"})
rule("function.core:octet_length", duck={"query": "SELECT octet_length(encode('héllo'))"})
rule("function.core:quote", duck=none("DuckDB has no function that writes a value as an SQL literal"))
rule("function.math:atan2", pg={"query": "SELECT round(atan2(1, 2)::numeric, 6)"})
rule("function.aggregate:total", pg={"query": "SELECT coalesce(sum(c), 0)::float8, coalesce(sum(NULL::float8), 0)::float8 FROM t"},
     duck={"query": "SELECT coalesce(sum(c), 0)::DOUBLE, coalesce(sum(NULL::DOUBLE), 0)::DOUBLE FROM t"})
rule("function.aggregate:percentile", pg={"query": "SELECT percentile_cont(0.5) WITHIN GROUP (ORDER BY c) FROM t"}, duck={"query": "SELECT quantile_cont(c, 0.5) FROM t"})

# Date and time: PostgreSQL's interval arithmetic and to_char, DuckDB's intervals and strftime.
# Where SQLite's arithmetic differs (a month added to January 31st carries into March; PostgreSQL
# and DuckDB stop at the month's end), the spelling says so in its answer, not by changing it.
TS = "'YYYY-MM-DD HH24:MI:SS'"
rule("function.datetime:date", pg={"query": "SELECT (date '2024-02-28' + interval '1 day')::date::text, (to_timestamp(0) AT TIME ZONE 'UTC')::date::text"},
     duck={"query": "SELECT (DATE '2024-02-28' + INTERVAL 1 DAY)::DATE::VARCHAR, epoch_ms(0)::DATE::VARCHAR"})
rule("function.datetime:time", pg={"query": "SELECT (timestamp '2024-01-01 13:45:30')::time::text, (time '13:45' + interval '30 minutes')::text"},
     duck={"query": "SELECT (TIMESTAMP '2024-01-01 13:45:30')::TIME::VARCHAR, (TIME '13:45' + INTERVAL 30 MINUTE)::VARCHAR"})
rule("function.datetime:datetime", pg={"query": f"SELECT to_char(timestamp '2024-01-31 10:00:00' + interval '1 month', {TS}), to_char(to_timestamp(1700000000) AT TIME ZONE 'UTC', {TS})"},
     duck={"query": "SELECT strftime(TIMESTAMP '2024-01-31 10:00:00' + INTERVAL 1 MONTH, '%Y-%m-%d %H:%M:%S'), strftime(epoch_ms(1700000000000), '%Y-%m-%d %H:%M:%S')"})
rule("function.datetime:julianday", pg={"query": "SELECT extract(julian from timestamp '2000-01-01 12:00:00') - 0.5"}, duck={"query": "SELECT julian(TIMESTAMP '2000-01-01 12:00:00')"})
rule("function.datetime:unixepoch", pg={"query": "SELECT extract(epoch from timestamp '2024-01-01 00:00:00')::bigint"}, duck={"query": "SELECT epoch(TIMESTAMP '2024-01-01 00:00:00')::BIGINT"})
rule("function.datetime:timediff", pg={"query": "SELECT to_char(age(timestamp '2024-03-15', timestamp '2024-01-01'), '\"+\"YYYY-MM-DD HH24:MI:SS.MS')"},
     duck={"query": "SELECT age(TIMESTAMP '2024-03-15', TIMESTAMP '2024-01-01')::VARCHAR"})
MODS = {
    "NNN days": ("SELECT (date '2024-01-01' + 10)::text", "SELECT (DATE '2024-01-01' + INTERVAL 10 DAY)::DATE::VARCHAR"),
    "NNN hours": (f"SELECT to_char(timestamp '2024-01-01' + interval '5 hours', {TS})", "SELECT strftime(TIMESTAMP '2024-01-01' + INTERVAL 5 HOUR, '%Y-%m-%d %H:%M:%S')"),
    "NNN minutes": (f"SELECT to_char(timestamp '2024-01-01' - interval '90 minutes', {TS})", "SELECT strftime(TIMESTAMP '2024-01-01' - INTERVAL 90 MINUTE, '%Y-%m-%d %H:%M:%S')"),
    "NNN seconds": (f"SELECT to_char(timestamp '2024-01-01' + interval '3600 seconds', {TS})", "SELECT strftime(TIMESTAMP '2024-01-01' + INTERVAL 3600 SECOND, '%Y-%m-%d %H:%M:%S')"),
    "NNN months": ("SELECT (date '2024-01-31' + interval '1 month')::date::text", "SELECT (DATE '2024-01-31' + INTERVAL 1 MONTH)::DATE::VARCHAR"),
    "NNN years": ("SELECT (date '2024-02-29' + interval '1 year')::date::text", "SELECT (DATE '2024-02-29' + INTERVAL 1 YEAR)::DATE::VARCHAR"),
    "start of month": ("SELECT date_trunc('month', date '2024-03-15')::date::text", "SELECT date_trunc('month', DATE '2024-03-15')::DATE::VARCHAR"),
    "start of year": ("SELECT date_trunc('year', date '2024-03-15')::date::text", "SELECT date_trunc('year', DATE '2024-03-15')::DATE::VARCHAR"),
    "start of day": (f"SELECT to_char(date_trunc('day', timestamp '2024-03-15 10:11:12'), {TS})", "SELECT strftime(date_trunc('day', TIMESTAMP '2024-03-15 10:11:12'), '%Y-%m-%d %H:%M:%S')"),
    "weekday N": ("SELECT (date '2024-03-15' + (7 - extract(dow from date '2024-03-15')::int) % 7)::text", "SELECT (DATE '2024-03-15' + INTERVAL ((7 - dayofweek(DATE '2024-03-15')) % 7) DAY)::DATE::VARCHAR"),
    "unixepoch": (f"SELECT to_char(to_timestamp(1710460800) AT TIME ZONE 'UTC', {TS})", "SELECT strftime(epoch_ms(1710460800000), '%Y-%m-%d %H:%M:%S')"),
    "julianday": ("SELECT (timestamp 'epoch' + (2460385.5 - 2440587.5) * interval '1 day')::date::text", "SELECT (TIMESTAMP '1970-01-01' + INTERVAL (2460385.5 - 2440587.5) DAY)::DATE::VARCHAR"),
    "auto": (f"SELECT to_char(to_timestamp(1710460800) AT TIME ZONE 'UTC', {TS})", "SELECT strftime(epoch_ms(1710460800000), '%Y-%m-%d %H:%M:%S')"),
    "localtime": (f"SELECT pg_typeof(to_char(timestamptz '2024-03-15 00:00:00+00' AT TIME ZONE current_setting('TimeZone'), {TS}))::text", "SELECT typeof(strftime(TIMESTAMPTZ '2024-03-15 00:00:00+00', '%Y-%m-%d %H:%M:%S'))"),
    "utc": (f"SELECT pg_typeof(to_char(timestamp '2024-03-15 00:00:00' AT TIME ZONE 'UTC', {TS}))::text", "SELECT typeof(strftime(TIMESTAMP '2024-03-15 00:00:00', '%Y-%m-%d %H:%M:%S'))"),
    "subsec": ("SELECT pg_typeof(to_char(clock_timestamp(), 'YYYY-MM-DD HH24:MI:SS.MS'))::text", "SELECT typeof(strftime(now(), '%Y-%m-%d %H:%M:%S.%g'))"),
    "subsecond": ("SELECT pg_typeof(to_char(clock_timestamp(), 'YYYY-MM-DD HH24:MI:SS.MS'))::text", "SELECT typeof(strftime(now(), '%Y-%m-%d %H:%M:%S.%g'))"),
    # 'ceiling' carries into the next month as SQLite always has; 'floor' stops at the month's end.
    "ceiling": ("SELECT (date_trunc('month', date '2024-01-31') + interval '1 month' + (extract(day from date '2024-01-31')::int - 1) * interval '1 day')::date::text",
                "SELECT (date_trunc('month', DATE '2024-01-31') + INTERVAL 1 MONTH + INTERVAL (day(DATE '2024-01-31') - 1) DAY)::DATE::VARCHAR"),
    "floor": ("SELECT (date '2024-01-31' + interval '1 month')::date::text", "SELECT (DATE '2024-01-31' + INTERVAL 1 MONTH)::DATE::VARCHAR"),
}
for name, (pg_q, duck_q) in MODS.items():
    rule(f"datetime.modifier:{name}", pg={"query": pg_q}, duck={"query": duck_q})

# JSON: PostgreSQL's jsonb functions and operators, DuckDB's json extension.
rule("function.json:json_array_length", pg={"query": f"SELECT jsonb_array_length({JB} -> 'b')"})
rule("function.json:json_each", pg={"query": f"SELECT key, value, jsonb_typeof(value) FROM jsonb_each({JB}) ORDER BY key"})
rule("function.json:json_error_position", pg=none("PostgreSQL tells valid JSON from invalid (IS JSON) but not where it breaks"),
     duck=none("DuckDB tells valid JSON from invalid (json_valid) but not where it breaks"))
rule("function.json:json_insert", pg={"query": f"SELECT ({JB} || jsonb_build_object('e', 5))::text"})
rule("function.json:json_patch", pg={"query": "SELECT ('{\"a\":1,\"b\":2}'::jsonb || '{\"b\":null,\"c\":3}'::jsonb)::text"},
     duck={"query": "SELECT json_merge_patch('{\"a\":1,\"b\":2}', '{\"b\":null,\"c\":3}')"})
rule("function.json:json_pretty", pg={"query": "SELECT jsonb_pretty('{\"a\":[1,2]}'::jsonb)"})
rule("function.json:json_quote", pg={"query": "SELECT to_json('a\"b'::text)::text, to_json(3)::text"})
rule("function.json:json_remove", pg={"query": f"SELECT ({JB} - 'b' - 'd')::text"})
rule("function.json:json_replace", pg={"query": f"SELECT jsonb_set(jsonb_set({JB}, '{{a}}', '7', false), '{{zz}}', '1', false)::text"})
rule("function.json:json_set", pg={"query": f"SELECT jsonb_set(jsonb_set({JB}, '{{a}}', '8'), '{{e}}', '\"n\"')::text"})
rule("function.json:json_tree", pg=none("PostgreSQL has no recursive JSON walk with SQLite's fullkey and type columns (jsonb_path_query('$.**') walks values only)"),
     duck=none("DuckDB has no recursive JSON walk with SQLite's fullkey and type columns"))
rule("function.json:json_type", pg={"query": f"SELECT jsonb_typeof({JB} -> 'b'), jsonb_typeof({JB} -> 'd')"})
rule("function.json:json_valid", pg={"query": "SELECT '{\"a\":1}' IS JSON, '{a:1}' IS JSON, '{a:1}' IS JSON"},
     duck={"query": "SELECT json_valid('{\"a\":1}'), json_valid('{a:1}'), json_valid('{a:1}')"})
rule("function.json:json_array_insert", pg={"query": "SELECT jsonb_insert('[1,3]'::jsonb, '{1}', '2')::text"},
     duck=none("DuckDB has no function that inserts into a JSON array"))
rule("function.json:json_group_array", duck={"query": "SELECT json_group_array(a) FROM (SELECT a FROM t ORDER BY a)"})
rule("function.json:jsonb", pg={"query": "SELECT '{\"a\":1}'::jsonb::text"}, duck={"query": "SELECT json('{\"a\":1}')"})
rule("function.json:jsonb_array", pg={"query": "SELECT jsonb_build_array(1, 2)::text"}, duck={"query": "SELECT json_array(1, 2)"})
rule("function.json:jsonb_array_insert", pg={"query": "SELECT jsonb_insert('[1,3]'::jsonb, '{1}', '2')::text"}, duck=none("DuckDB has no function that inserts into a JSON array"))
rule("function.json:jsonb_each", pg={"query": f"SELECT key FROM jsonb_each({JB}) ORDER BY key"}, duck={"query": f"SELECT key FROM json_each({J}) ORDER BY key"})
rule("function.json:jsonb_extract", pg={"query": f"SELECT ({JB} -> 'b')::text"}, duck={"query": f"SELECT json_extract({J}, '$.b')"})
rule("function.json:jsonb_group_array", pg={"query": "SELECT jsonb_agg(a ORDER BY a)::text FROM t"}, duck={"query": "SELECT json_group_array(a) FROM (SELECT a FROM t ORDER BY a)"})
rule("function.json:jsonb_group_object", pg={"query": "SELECT jsonb_object_agg(b, a)::text FROM t WHERE b IS NOT NULL"},
     duck={"query": "SELECT json_group_object(b, a) FROM (SELECT a, b FROM t WHERE b IS NOT NULL ORDER BY a)"})
rule("function.json:jsonb_insert", pg={"query": f"SELECT ({JB} || jsonb_build_object('e', 5))::text"}, duck=none("DuckDB has no JSON insert function"))
rule("function.json:jsonb_object", pg={"query": "SELECT jsonb_build_object('k', 1)::text"}, duck={"query": "SELECT json_object('k', 1)"})
rule("function.json:jsonb_patch", pg={"query": "SELECT ('{\"a\":1}'::jsonb || '{\"b\":2}'::jsonb)::text"}, duck={"query": "SELECT json_merge_patch('{\"a\":1}', '{\"b\":2}')"})
rule("function.json:jsonb_remove", pg={"query": f"SELECT ({JB} - 'b')::text"}, duck=none("DuckDB has no JSON remove function"))
rule("function.json:jsonb_replace", pg={"query": f"SELECT jsonb_set({JB}, '{{a}}', '2', false)::text"}, duck=none("DuckDB has no JSON replace function"))
rule("function.json:jsonb_set", pg={"query": f"SELECT jsonb_set({JB}, '{{a}}', '3')::text"}, duck=none("DuckDB has no JSON set function"))
rule("function.json:jsonb_tree", pg=none("PostgreSQL has no recursive JSON walk with SQLite's columns"), duck=none("DuckDB has no recursive JSON walk with SQLite's columns"))
for name in ["json_insert", "json_remove", "json_replace", "json_set"]:
    rule(f"function.json:{name}", duck=none(f"DuckDB's json extension has no {name}() (json_merge_patch and json_transform reshape documents instead)"))

# Operators and expressions.
rule("operator:- (unary)", pg={"query": "SELECT -(3), -('4'::numeric)"}, duck={"query": "SELECT -(3), -('4'::INTEGER)"})
rule("operator:IS", pg={"query": "SELECT NULL IS NOT DISTINCT FROM NULL, 1 IS NOT DISTINCT FROM 1"}, duck={"query": "SELECT NULL IS NOT DISTINCT FROM NULL, 1 IS NOT DISTINCT FROM 1"},
     my={"query": "SELECT NULL <=> NULL, 1 <=> 1"}, ch={"query": "SELECT isNotDistinctFrom(NULL, NULL), isNotDistinctFrom(1, 1)"})
rule("operator:CAST#3", pg=none("PostgreSQL refuses text that isn't a whole number rather than reading its numeric prefix"),
     duck=none("DuckDB refuses text that isn't a whole number rather than reading its numeric prefix"))
NO_TRUTH = "a condition must be boolean there: text has no truth value"
rule("expression:boolean expression", pg=none("PostgreSQL: " + NO_TRUTH), duck=none("DuckDB: " + NO_TRUTH))
rule("word:TRUE", pg={"query": "SELECT TRUE, TRUE::int + 1"}, duck={"query": "SELECT TRUE, TRUE::INTEGER + 1"})
rule("word:STORED", duck=none("DuckDB's generated columns are virtual only"))
rule("keyword:FAIL", pg={"query": "INSERT INTO kp VALUES (2); SELECT count(*) FROM kp"}, my={"query": "INSERT INTO kp VALUES (2); SELECT count(*) FROM kp"},
     duck={"query": "INSERT INTO kp VALUES (2); SELECT count(*) FROM kp"}, ch={"query": "INSERT INTO kp VALUES (2); SELECT count(*) FROM kp"})

# Statements and clauses.
rule("syntax:table-or-subquery#2", pg={"query": "SELECT value FROM jsonb_array_elements('[3,1,2]'::jsonb) AS e(value) ORDER BY value::int"})
rule("syntax:upsert-clause#1", pg={"query": "INSERT INTO pk VALUES (1, 'up') ON CONFLICT (k) DO UPDATE SET v = excluded.v || '!' RETURNING k, v"},
     duck={"query": "INSERT INTO pk VALUES (1, 'up') ON CONFLICT (k) DO UPDATE SET v = excluded.v || '!' RETURNING k, v"})
rule("syntax:update-stmt-limited", pg={"query": "UPDATE t SET b = 'lim' WHERE ctid IN (SELECT ctid FROM t WHERE a > 0 ORDER BY a LIMIT 2)"},
     duck={"query": "UPDATE t SET b = 'lim' WHERE rowid IN (SELECT rowid FROM t WHERE a > 0 ORDER BY a LIMIT 2)"},
     ch=none("ClickHouse's ALTER TABLE ... UPDATE takes no ORDER BY or LIMIT"))
rule("syntax:delete-stmt-limited", pg={"query": "DELETE FROM t WHERE ctid IN (SELECT ctid FROM t WHERE a > 0 ORDER BY a DESC LIMIT 1)"},
     duck={"query": "DELETE FROM t WHERE rowid IN (SELECT rowid FROM t WHERE a > 0 ORDER BY a DESC LIMIT 1)"},
     ch=none("ClickHouse's DELETE takes no ORDER BY or LIMIT"))
rule("syntax:table-constraint#2", duck=none("DuckDB checks constraints at once: it has no deferrable constraints"))
rule("syntax:foreign-key-clause", pg={"query": "CREATE TABLE fk(a INT REFERENCES pk(k) MATCH SIMPLE ON UPDATE SET NULL ON DELETE SET DEFAULT)"},
     duck=none("DuckDB's foreign keys take no ON UPDATE or ON DELETE actions"))
rule("syntax:create-view-stmt#1", pg={"query": "CREATE OR REPLACE VIEW v1(x, y) AS SELECT a, b FROM t"})
rule("syntax:drop-trigger-stmt", pg={"query": "DROP TRIGGER IF EXISTS nope ON t"}, duck=none("DuckDB has no triggers"), ch=none("ClickHouse has no triggers"))
rule("syntax:commit-stmt#2", pg={"setup_tail": ["BEGIN"], "query": "END TRANSACTION"}, duck={"setup_tail": ["BEGIN TRANSACTION"], "query": "END TRANSACTION"},
     my={"setup_tail": ["START TRANSACTION"], "query": "COMMIT"})
rule("syntax:reindex-stmt#1", pg={"query": "REINDEX DATABASE postgres"})
rule("syntax:create-index-stmt#2", duck=none("DuckDB has no partial indexes"))
rule("syntax:column-name-list", duck={"query": "UPDATE t SET a = 1, b = 'z' WHERE a = 4"})
rule("syntax:pragma-stmt#1",
     pg={"query": "SELECT ordinal_position - 1 AS cid, column_name AS name, upper(data_type) AS type, (is_nullable = 'NO')::int AS notnull, column_default AS dflt_value, 0 AS pk FROM information_schema.columns WHERE table_name = 't' AND table_schema = current_schema()"},
     duck={"query": "PRAGMA table_info('t')"})
rule("syntax:pragma-stmt#2", pg=none("PostgreSQL keeps no user version number in a database"), my=none("MySQL keeps no user version number in a database"),
     duck=none("DuckDB keeps no user version number in a database"), ch=none("ClickHouse keeps no user version number in a database"))
rule("syntax:pragma-value", pg=none("PostgreSQL sizes its cache in the server's configuration (shared_buffers)"), my=none("MySQL sizes its cache in the server's configuration (innodb_buffer_pool_size)"),
     duck={"query": "SET memory_limit = '2GB'"}, ch=none("ClickHouse sizes its caches in the server's configuration"))
rule("syntax:literal-value", pg={"query": "SELECT 1, 1.5, 'txt', '\\x0f'::bytea, NULL, TRUE, FALSE, pg_typeof(CURRENT_DATE)::text, pg_typeof(CURRENT_TIME)::text, pg_typeof(CURRENT_TIMESTAMP)::text"})
rule("syntax:sql-stmt#1", ch={"query": "EXPLAIN SELECT 1"})


def add_collations(probes):
    """Each probe that says COLLATE NOCASE or BINARY, in PostgreSQL with a case-insensitive ICU
    collation made for it (and "C" for BINARY), and in MySQL with its _ci and _bin collations."""
    import re
    import sqlglot
    targets = {PG: ({"NOCASE": "nocase", "BINARY": '"C"'}, ["CREATE COLLATION nocase (provider = icu, locale = 'und-u-ks-level2', deterministic = false)"]),
               MY: ({"NOCASE": "utf8mb4_0900_ai_ci", "BINARY": "utf8mb4_bin"}, [])}
    for p in probes:
        text = " ".join(p["setup"] + [p["query"]])
        if not re.search(r"COLLATE\s+(NOCASE|BINARY)", text, re.I) or re.search(r"COLLATE\s+RTRIM", text, re.I):
            continue
        for d, (names, prelude) in targets.items():
            if p["id"] in RULES and d in RULES[p["id"]]:
                continue
            try:
                own = p["setup"][p.get("base_len", 0):]
                tail = [";\n".join(sqlglot.transpile(s, read="sqlite", write=d)) for s in own]
                query = ";\n".join(sqlglot.transpile(p["query"], read="sqlite", write=d))
            except Exception:
                continue
            swap = lambda s: re.sub(r"COLLATE\s+(NOCASE|BINARY)\b", lambda m: "COLLATE " + names[m.group(1).upper()], s, flags=re.I)
            RULES.setdefault(p["id"], {})[d] = {"setup_tail": prelude + [swap(s) for s in tail], "query": swap(query)}

# ---------------------------------------------------------------- MySQL
MYTS = "'%Y-%m-%d %H:%i:%s'"
rule("function.core:printf", my={"query": "SELECT CONCAT(LPAD(FORMAT(3.14159, 2), 5, ' '), '|', RPAD('a', 3, ' '), '|', LPAD(7, 3, '0'))"})
rule("function.core:quote", my={"query": "SELECT QUOTE('it''s'), QUOTE(NULL), QUOTE(HEX(x'ab'))"})
rule("function.core:random", my={"query": "SELECT RAND() >= 0"})
rule("function.core:unhex", my={"query": "SELECT UNHEX('4142') = BINARY 'AB'"})
rule("function.core:unistr", my=none("MySQL has no unistr(); CHAR(... USING utf8mb4) builds text from code points"))
rule("function.core:load_extension", my=none("MySQL loads plugins with INSTALL PLUGIN, a statement"))
for name, formula in [("acosh", "LN(2 + SQRT(2 * 2 - 1))"), ("asinh", "LN(1 + SQRT(1 * 1 + 1))"), ("atanh", "0.5 * LN((1 + 0.5) / (1 - 0.5))"),
                      ("cosh", "(EXP(1) + EXP(-1)) / 2"), ("sinh", "(EXP(1) - EXP(-1)) / 2"), ("tanh", "(EXP(1) - EXP(-1)) / (EXP(1) + EXP(-1))")]:
    rule(f"function.math:{name}", my={"query": f"SELECT ROUND({formula}, 6)"})
rule("function.datetime:date", my={"query": "SELECT DATE_FORMAT(DATE_ADD('2024-02-28', INTERVAL 1 DAY), '%Y-%m-%d'), DATE_FORMAT(FROM_UNIXTIME(0), '%Y-%m-%d')"})
rule("function.datetime:time", my={"query": "SELECT TIME_FORMAT(TIME('2024-01-01 13:45:30'), '%H:%i:%s'), TIME_FORMAT(ADDTIME('13:45:00', '00:30:00'), '%H:%i:%s')"})
rule("function.datetime:datetime", my={"query": f"SELECT DATE_FORMAT(DATE_ADD('2024-01-31 10:00:00', INTERVAL 1 MONTH), {MYTS}), DATE_FORMAT(FROM_UNIXTIME(1700000000), {MYTS})"})
rule("function.datetime:julianday", my={"query": "SELECT TO_DAYS('2000-01-01 12:00:00') + 1721059.5 + TIME_TO_SEC(TIME('2000-01-01 12:00:00')) / 86400"})
rule("function.datetime:unixepoch", my={"query": "SELECT UNIX_TIMESTAMP('2024-01-01 00:00:00')"})
MY_MODS = {
    "NNN days": "SELECT DATE_FORMAT(DATE_ADD('2024-01-01', INTERVAL 10 DAY), '%Y-%m-%d')",
    "NNN hours": f"SELECT DATE_FORMAT(DATE_ADD('2024-01-01', INTERVAL 5 HOUR), {MYTS})",
    "NNN minutes": f"SELECT DATE_FORMAT(DATE_SUB('2024-01-01', INTERVAL 90 MINUTE), {MYTS})",
    "NNN seconds": f"SELECT DATE_FORMAT(DATE_ADD('2024-01-01', INTERVAL 3600 SECOND), {MYTS})",
    "NNN months": "SELECT DATE_FORMAT(DATE_ADD('2024-01-31', INTERVAL 1 MONTH), '%Y-%m-%d')",
    "NNN years": "SELECT DATE_FORMAT(DATE_ADD('2024-02-29', INTERVAL 1 YEAR), '%Y-%m-%d')",
    "start of month": "SELECT DATE_FORMAT('2024-03-15', '%Y-%m-01')",
    "start of year": "SELECT DATE_FORMAT('2024-03-15', '%Y-01-01')",
    "start of day": "SELECT DATE_FORMAT('2024-03-15 10:11:12', '%Y-%m-%d 00:00:00')",
    "weekday N": "SELECT DATE_FORMAT(DATE_ADD('2024-03-15', INTERVAL (8 - DAYOFWEEK('2024-03-15')) % 7 DAY), '%Y-%m-%d')",
    "unixepoch": f"SELECT DATE_FORMAT(FROM_UNIXTIME(1710460800), {MYTS})",
    "julianday": "SELECT DATE_FORMAT(FROM_DAYS(FLOOR(2460385.5 - 1721059.5)), '%Y-%m-%d')",
    "auto": f"SELECT DATE_FORMAT(FROM_UNIXTIME(1710460800), {MYTS})",
    "localtime": f"SELECT DATE_FORMAT(CONVERT_TZ('2024-03-15 00:00:00', '+00:00', @@session.time_zone), {MYTS}) IS NOT NULL",
    "utc": f"SELECT DATE_FORMAT(CONVERT_TZ('2024-03-15 00:00:00', @@session.time_zone, '+00:00'), {MYTS}) IS NOT NULL",
    "subsec": "SELECT DATE_FORMAT(NOW(3), '%Y-%m-%d %H:%i:%s.%f') IS NOT NULL",
    "subsecond": "SELECT DATE_FORMAT(NOW(3), '%Y-%m-%d %H:%i:%s.%f') IS NOT NULL",
    "ceiling": "SELECT DATE_FORMAT(DATE_ADD(DATE_ADD(DATE_FORMAT('2024-01-31', '%Y-%m-01'), INTERVAL 1 MONTH), INTERVAL DAY('2024-01-31') - 1 DAY), '%Y-%m-%d')",
    "floor": "SELECT DATE_FORMAT(DATE_ADD('2024-01-31', INTERVAL 1 MONTH), '%Y-%m-%d')",
}
for name, q in MY_MODS.items():
    rule(f"datetime.modifier:{name}", my={"query": q})
rule("function.json:json", my={"query": "SELECT CAST(' {\"x\" : 1} ' AS JSON)"})
rule("function.json:json_array_length", my={"query": f"SELECT JSON_LENGTH({J}, '$.b')"})
rule("function.json:json_each", my={"query": f"SELECT jt.k, JSON_EXTRACT({J}, CONCAT('$.', jt.k)), LOWER(JSON_TYPE(JSON_EXTRACT({J}, CONCAT('$.', jt.k)))) FROM JSON_TABLE(JSON_KEYS({J}), '$[*]' COLUMNS (k VARCHAR(16) PATH '$')) AS jt ORDER BY jt.k"})
rule("function.json:json_error_position", my=none("MySQL tells valid JSON from invalid (JSON_VALID) but not where it breaks"))
rule("function.json:json_extract", my={"query": f"SELECT JSON_UNQUOTE(JSON_EXTRACT({J}, '$.b[2].c')), JSON_EXTRACT({J}, '$.a', '$.d')"})
rule("function.json:json_group_array", my={"query": "SELECT JSON_ARRAYAGG(a) FROM (SELECT a FROM t ORDER BY a) AS s"})
rule("function.json:json_group_object", my={"query": "SELECT JSON_OBJECTAGG(b, a) FROM (SELECT a, b FROM t WHERE b IS NOT NULL ORDER BY a) AS s"})
rule("function.json:json_patch", my={"query": "SELECT JSON_MERGE_PATCH('{\"a\":1,\"b\":2}', '{\"b\":null,\"c\":3}')"})
rule("function.json:json_quote", my={"query": "SELECT JSON_QUOTE('a\"b'), CAST(3 AS JSON)"})
rule("function.json:json_tree", my=none("MySQL has no recursive JSON walk with SQLite's fullkey and type columns"))
rule("function.json:json_type", my={"query": f"SELECT LOWER(JSON_TYPE(JSON_EXTRACT({J}, '$.b'))), LOWER(JSON_TYPE(JSON_EXTRACT({J}, '$.d')))"})
rule("function.json:json_valid", my={"query": "SELECT JSON_VALID('{\"a\":1}'), JSON_VALID('{a:1}'), JSON_VALID('{a:1}')"})
rule("function.json:json_array_insert", my={"query": "SELECT JSON_ARRAY_INSERT('[1,3]', '$[1]', 2)"})
rule("function.json:jsonb", my={"query": "SELECT CAST('{\"a\":1}' AS JSON)"})
rule("function.json:jsonb_array", my={"query": "SELECT JSON_ARRAY(1, 2)"})
rule("function.json:jsonb_array_insert", my={"query": "SELECT JSON_ARRAY_INSERT('[1,3]', '$[1]', 2)"})
rule("function.json:jsonb_each", my={"query": f"SELECT jt.k FROM JSON_TABLE(JSON_KEYS({J}), '$[*]' COLUMNS (k VARCHAR(16) PATH '$')) AS jt ORDER BY jt.k"})
rule("function.json:jsonb_extract", my={"query": f"SELECT JSON_EXTRACT({J}, '$.b')"})
rule("function.json:jsonb_group_array", my={"query": "SELECT JSON_ARRAYAGG(a) FROM (SELECT a FROM t ORDER BY a) AS s"})
rule("function.json:jsonb_group_object", my={"query": "SELECT JSON_OBJECTAGG(b, a) FROM (SELECT a, b FROM t WHERE b IS NOT NULL ORDER BY a) AS s"})
rule("function.json:jsonb_insert", my={"query": f"SELECT JSON_INSERT({J}, '$.e', 5)"})
rule("function.json:jsonb_object", my={"query": "SELECT JSON_OBJECT('k', 1)"})
rule("function.json:jsonb_patch", my={"query": "SELECT JSON_MERGE_PATCH('{\"a\":1}', '{\"b\":2}')"})
rule("function.json:jsonb_remove", my={"query": f"SELECT JSON_REMOVE({J}, '$.b')"})
rule("function.json:jsonb_replace", my={"query": f"SELECT JSON_REPLACE({J}, '$.a', 2)"})
rule("function.json:jsonb_set", my={"query": f"SELECT JSON_SET({J}, '$.a', 3)"})
rule("function.json:jsonb_tree", my=none("MySQL has no recursive JSON walk with SQLite's columns"))
rule("function.aggregate:total", my={"query": "SELECT COALESCE(SUM(c), 0) + 0.0, 0.0 FROM t"})
NO_PERCENTILE = "MySQL has no percentile or median aggregate (PERCENT_RANK and CUME_DIST are window functions)"
for name in ["median", "percentile", "percentile_cont", "percentile_disc"]:
    rule(f"function.aggregate:{name}", my=none(NO_PERCENTILE))
FILTER_MY = {"syntax:filter-clause#1": "SELECT SUM(CASE WHEN b IS NULL THEN 1 ELSE 0 END) FROM t",
             "syntax:filter-clause#2": "SELECT SUM(CASE WHEN a > 2 THEN 1 ELSE 0 END), SUM(CASE WHEN b IS NOT NULL THEN a END) FROM t",
             "syntax:frame-spec#aggregate-filter": "SELECT a, SUM(CASE WHEN a > 2 THEN 1 ELSE 0 END) OVER (ORDER BY a) FROM t ORDER BY a",
             "syntax:window-function-invocation": "SELECT a, SUM(CASE WHEN a > 1 THEN a END) OVER (ORDER BY a) FROM t ORDER BY a"}
for pid, q in FILTER_MY.items():
    rule(pid, my={"query": q})
NO_EXCLUDE = "{} has no EXCLUDE clause in a window frame"
for v in ["exclude-current-row", "exclude-group", "exclude-ties"]:
    rule(f"syntax:frame-spec#{v}", my=none(NO_EXCLUDE.format("MySQL")), ch=none(NO_EXCLUDE.format("ClickHouse")))
rule("keyword:OTHERS", my={"query": "SELECT a, SUM(a) OVER (ORDER BY a ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING) FROM t ORDER BY a"},
     ch={"query": "SELECT a, sum(a) OVER (ORDER BY a ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING) FROM t ORDER BY a"})
rule("syntax:frame-spec#groups", my=none("MySQL has no GROUPS frames"))
rule("operator:->>", my={"query": "SELECT JSON_UNQUOTE(JSON_EXTRACT('{\"a\":{\"b\":2}}', '$.a.b'))"})
rule("syntax:select-core#2", my={"query": "VALUES ROW(1, 'a'), ROW(2, 'b')"}, ch={"query": "SELECT 1, 'a' UNION ALL SELECT 2, 'b'"})
NO_HINT = "{} takes no materialization hint on a CTE: its optimizer decides"
rule("syntax:common-table-expression#1", my=none(NO_HINT.format("MySQL")))
rule("syntax:common-table-expression#2", my=none(NO_HINT.format("MySQL")), ch=none(NO_HINT.format("ClickHouse")))
rule("syntax:join-clause#4", my={"query": "SELECT a, d FROM (SELECT t.a AS a, u.d AS d, COALESCE(t.a, u.a) AS k FROM t LEFT JOIN u ON t.a = u.a UNION ALL SELECT t.a, u.d, u.a FROM t RIGHT JOIN u ON t.a = u.a WHERE t.a IS NULL) AS f ORDER BY k, d"})
rule("syntax:table-or-subquery#2", my={"query": "SELECT value FROM JSON_TABLE('[3,1,2]', '$[*]' COLUMNS (value INT PATH '$')) AS j ORDER BY value"},
     ch={"query": "SELECT arrayJoin(JSONExtract('[3,1,2]', 'Array(Int64)')) AS value ORDER BY value"})
NO_RETURNING = "{} returns no rows from a change (RETURNING is not in its grammar)"
rule("syntax:insert-stmt#1", my=none(NO_RETURNING.format("MySQL")), ch=none(NO_RETURNING.format("ClickHouse")))
rule("syntax:insert-stmt#5", my={"query": "INSERT INTO t () VALUES ()"}, ch={"query": "INSERT INTO t SELECT NULL, NULL, NULL"})
rule("syntax:upsert-clause#1", my={"query": "INSERT INTO pk VALUES (1, 'up') ON DUPLICATE KEY UPDATE v = CONCAT(VALUES(v), '!'); SELECT k, v FROM pk WHERE k = 1"},
     ch=none("ClickHouse keeps no unique constraint to resolve a conflict on"))
rule("syntax:upsert-clause#2", my={"query": "INSERT IGNORE INTO pk VALUES (1, 'no')"}, ch=none("ClickHouse keeps no unique constraint to resolve a conflict on"))
rule("syntax:returning-clause#1", my=none(NO_RETURNING.format("MySQL")), ch=none(NO_RETURNING.format("ClickHouse")))
rule("syntax:returning-clause#2", my=none(NO_RETURNING.format("MySQL")), ch=none(NO_RETURNING.format("ClickHouse")))
rule("syntax:update-stmt#3", my={"query": "UPDATE t SET b = 'p', c = 9.5 WHERE a = 2"})
rule("syntax:column-name-list", my={"query": "UPDATE t SET a = 1, b = 'z' WHERE a = 4"})
rule("syntax:table-constraint#2", my=none("MySQL checks foreign keys at once: it has no deferrable constraints"))
rule("syntax:foreign-key-clause", my={"query": "CREATE TABLE fk(a INT, FOREIGN KEY (a) REFERENCES pk(k) ON UPDATE SET NULL ON DELETE SET DEFAULT)"})
rule("syntax:create-index-stmt#1", my={"query": "CREATE UNIQUE INDEX ix ON t(a DESC)"})
INDEX_COLLATE = "MySQL takes a column's collation from the column's definition, not from an index"
rule("syntax:create-index-stmt#4", my=none(INDEX_COLLATE))
rule("syntax:indexed-column", my=none(INDEX_COLLATE))
rule("syntax:create-index-stmt#2", my=none("MySQL has no partial indexes"))
rule("syntax:create-index-stmt#3", my={"query": "CREATE INDEX ex ON t((LOWER(b)))"})
rule("syntax:create-view-stmt#1", my={"query": "CREATE OR REPLACE VIEW v1 (x, y) AS SELECT a, b FROM t"})
rule("syntax:create-view-stmt#2", my=none("MySQL has no temporary views"))
rule("syntax:create-trigger-stmt#2", my=none("MySQL can't skip a row from a trigger (SQLite's RAISE(IGNORE))"))
rule("syntax:create-trigger-stmt#3", my={"query": "CREATE TRIGGER tu AFTER UPDATE ON t FOR EACH ROW UPDATE u SET d = NEW.b WHERE a = NEW.a"})
rule("syntax:create-trigger-stmt#4", my=none("MySQL has no INSTEAD OF triggers (no trigger on a view)"))
rule("syntax:drop-index-stmt", my={"query": "DROP INDEX ti ON t"})
rule("syntax:pragma-stmt#1", my={"query": "SELECT ordinal_position - 1 AS cid, column_name AS name, upper(data_type) AS type, (is_nullable = 'NO') AS notnull, column_default AS dflt_value, 0 AS pk FROM information_schema.columns WHERE table_name = 't' AND table_schema = database()"})
rule("syntax:literal-value", my={"query": "SELECT 1, 1.5, 'txt', x'0F', NULL, TRUE, FALSE, CURRENT_DATE IS NOT NULL, CURRENT_TIME IS NOT NULL, CURRENT_TIMESTAMP IS NOT NULL"})
rule("syntax:create-table-stmt#1", my={"query": "CREATE TABLE IF NOT EXISTS ct (a INT PRIMARY KEY, b VARCHAR(255) NOT NULL DEFAULT 'x', c DOUBLE CHECK (c >= 0), UNIQUE (b))"})
rule("syntax:alter-table-stmt#3", my={"query": "ALTER TABLE t ADD COLUMN e VARCHAR(255) DEFAULT 'e'"})

# ---------------------------------------------------------------- ClickHouse
CHTS = "'%Y-%m-%d %H:%i:%S'"
rule("function.core:format", ch={"query": "SELECT format('{}-{}', 7, 'x')"})
rule("function.core:max", ch={"query": "SELECT greatest(1, 5, 3)"})
rule("function.core:min", ch={"query": "SELECT least(4, 2, 8)"})
rule("function.core:unistr", ch=none("ClickHouse has no unistr()"))
for name in ["acosh", "asinh", "atanh", "cosh", "sinh", "tanh"]:
    arg = {"acosh": "2", "asinh": "1", "atanh": "0.5"}.get(name, "1")
    rule(f"function.math:{name}", ch={"query": f"SELECT round({name}({arg}), 6)"})
rule("function.datetime:date", ch={"query": "SELECT toString(addDays(toDate('2024-02-28'), 1)), toString(toDate(toDateTime(0, 'UTC')))"})
rule("function.datetime:time", ch={"query": "SELECT formatDateTime(toDateTime('2024-01-01 13:45:30', 'UTC'), '%H:%i:%S'), formatDateTime(toDateTime('1970-01-01 13:45:00', 'UTC') + INTERVAL 30 MINUTE, '%H:%i:%S')"})
rule("function.datetime:datetime", ch={"query": f"SELECT formatDateTime(addMonths(toDateTime('2024-01-31 10:00:00', 'UTC'), 1), {CHTS}), formatDateTime(toDateTime(1700000000, 'UTC'), {CHTS})"})
rule("function.datetime:julianday", ch={"query": "SELECT toUnixTimestamp(toDateTime('2000-01-01 12:00:00', 'UTC')) / 86400 + 2440587.5"})
rule("function.datetime:unixepoch", ch={"query": "SELECT toUnixTimestamp(toDateTime('2024-01-01 00:00:00', 'UTC'))"})
rule("function.datetime:strftime", ch={"query": "SELECT formatDateTime(toDateTime('2024-03-15 08:05:00', 'UTC'), '%Y-%m-%d %H:%i %j %w')"})
rule("function.datetime:timediff", ch={"query": "SELECT dateDiff('day', toDate('2024-01-01'), toDate('2024-03-15'))"})
CH_MODS = {
    "NNN days": "SELECT toString(addDays(toDate('2024-01-01'), 10))",
    "NNN hours": f"SELECT formatDateTime(toDateTime('2024-01-01 00:00:00', 'UTC') + INTERVAL 5 HOUR, {CHTS})",
    "NNN minutes": f"SELECT formatDateTime(toDateTime('2024-01-01 00:00:00', 'UTC') - INTERVAL 90 MINUTE, {CHTS})",
    "NNN seconds": f"SELECT formatDateTime(toDateTime('2024-01-01 00:00:00', 'UTC') + INTERVAL 3600 SECOND, {CHTS})",
    "NNN months": "SELECT toString(addMonths(toDate('2024-01-31'), 1))",
    "NNN years": "SELECT toString(addYears(toDate('2024-02-29'), 1))",
    "start of month": "SELECT toString(toStartOfMonth(toDate('2024-03-15')))",
    "start of year": "SELECT toString(toStartOfYear(toDate('2024-03-15')))",
    "start of day": f"SELECT formatDateTime(toStartOfDay(toDateTime('2024-03-15 10:11:12', 'UTC')), {CHTS})",
    "weekday N": "SELECT toString(addDays(toDate('2024-03-15'), (7 - toDayOfWeek(toDate('2024-03-15'), 2)) % 7))",
    "unixepoch": f"SELECT formatDateTime(toDateTime(1710460800, 'UTC'), {CHTS})",
    "julianday": "SELECT toString(addDays(toDate('1970-01-01'), toInt32(2460385.5 - 2440587.5)))",
    "auto": f"SELECT formatDateTime(toDateTime(1710460800, 'UTC'), {CHTS})",
    "localtime": f"SELECT toTypeName(formatDateTime(toDateTime('2024-03-15 00:00:00', 'UTC'), {CHTS}))",
    "utc": f"SELECT toTypeName(formatDateTime(toDateTime('2024-03-15 00:00:00', 'UTC'), {CHTS}))",
    "subsec": "SELECT toTypeName(formatDateTime(now64(3), '%Y-%m-%d %H:%i:%S.%f'))",
    "subsecond": "SELECT toTypeName(formatDateTime(now64(3), '%Y-%m-%d %H:%i:%S.%f'))",
    "ceiling": "SELECT toString(addDays(addMonths(toStartOfMonth(toDate('2024-01-31')), 1), toDayOfMonth(toDate('2024-01-31')) - 1))",
    "floor": "SELECT toString(addMonths(toDate('2024-01-31'), 1))",
}
for name, q in CH_MODS.items():
    rule(f"datetime.modifier:{name}", ch={"query": q})
NO_JSON_EDIT = "ClickHouse has no function that edits a JSON document"
rule("function.json:json", ch={"query": "SELECT toString(' {\"x\" : 1} '::JSON)"})
rule("function.json:json_array", ch={"query": "SELECT toJSONString(tuple(1, 'a', NULL, 2.5))"})
rule("function.json:json_array_insert", ch=none(NO_JSON_EDIT))
rule("function.json:json_array_length", ch={"query": f"SELECT JSONLength({J}, 'b')"})
rule("function.json:json_each", ch={"query": f"SELECT kv.1 AS key, kv.2 AS value, lower(toString(JSONType({J}, kv.1))) FROM (SELECT arrayJoin(JSONExtractKeysAndValuesRaw({J})) AS kv) ORDER BY key"})
rule("function.json:json_error_position", ch=none("ClickHouse tells valid JSON from invalid (isValidJSON) but not where it breaks"))
rule("function.json:json_extract", ch={"query": f"SELECT JSONExtractInt({J}, 'b', 3, 'c'), concat('[', JSONExtractRaw({J}, 'a'), ',', JSONExtractRaw({J}, 'd'), ']')"})
rule("function.json:json_group_array", ch={"query": "SELECT toJSONString(groupArray(a)) FROM (SELECT a FROM t ORDER BY a)"})
rule("function.json:json_group_object", ch={"query": "SELECT toJSONString(CAST(groupArray(tuple(b, a)), 'Map(String, Int32)')) FROM (SELECT a, b FROM t WHERE b IS NOT NULL ORDER BY a)"})
for name in ["json_insert", "json_patch", "json_remove", "json_replace", "json_set"]:
    rule(f"function.json:{name}", ch=none(NO_JSON_EDIT))
rule("function.json:json_object", ch={"query": "SELECT toJSONString(CAST(tuple(1, 'x'), 'Tuple(k UInt8, v String)'))"})
rule("function.json:json_pretty", ch=none("ClickHouse has no JSON pretty-printer"))
rule("function.json:json_quote", ch={"query": "SELECT toJSONString('a\"b'), toJSONString(3)"})
rule("function.json:json_tree", ch=none("ClickHouse has no recursive JSON walk with SQLite's columns"))
rule("function.json:json_type", ch={"query": f"SELECT lower(toString(JSONType({J}, 'b'))), lower(toString(JSONType({J}, 'd')))"})
rule("function.json:json_valid", ch={"query": "SELECT isValidJSON('{\"a\":1}'), isValidJSON('{a:1}'), isValidJSON('{a:1}')"})
NO_JSONB = "ClickHouse has no binary JSON functions (its JSON type keeps paths as columns)"
for name in ["jsonb", "jsonb_array_insert", "jsonb_each", "jsonb_insert", "jsonb_patch", "jsonb_remove", "jsonb_replace", "jsonb_set", "jsonb_tree"]:
    rule(f"function.json:{name}", ch=none(NO_JSONB))
rule("function.json:jsonb_array", ch={"query": "SELECT toJSONString(tuple(1, 2))"})
rule("function.json:jsonb_extract", ch={"query": f"SELECT JSONExtractRaw({J}, 'b')"})
rule("function.json:jsonb_group_array", ch={"query": "SELECT toJSONString(groupArray(a)) FROM (SELECT a FROM t ORDER BY a)"})
rule("function.json:jsonb_group_object", ch={"query": "SELECT toJSONString(CAST(groupArray(tuple(b, a)), 'Map(String, Int32)')) FROM (SELECT a, b FROM t WHERE b IS NOT NULL ORDER BY a)"})
rule("function.json:jsonb_object", ch={"query": "SELECT toJSONString(CAST(tuple(1), 'Tuple(k UInt8)'))"})
CONCAT_ORDERED = "SELECT arrayStringConcat(arrayMap(x -> x.2, {}(x -> x.1, groupArray(tuple(a, b)))), '{}') FROM t WHERE b IS NOT NULL"
rule("function.aggregate:group_concat", ch={"query": CONCAT_ORDERED.format("arraySort", "|")})
rule("function.aggregate:string_agg", ch={"query": CONCAT_ORDERED.format("arrayReverseSort", ",")})
rule("syntax:aggregate-function-invocation#2", ch={"query": CONCAT_ORDERED.format("arrayReverseSort", ",")})
rule("function.aggregate:total", ch={"query": "SELECT toFloat64(ifNull(sum(c), 0)), toFloat64(0) FROM t"})
rule("function.aggregate:percentile", ch={"query": "SELECT quantileExactInclusive(0.5)(c) FROM t"})
rule("function.aggregate:percentile_cont", ch={"query": "SELECT quantileExactInclusive(0.25)(c) FROM t"})
rule("function.aggregate:percentile_disc", ch={"query": "SELECT quantileExactLow(0.25)(c) FROM t"})
rule("function.window:cume_dist", ch={"query": "SELECT a, (count(*) OVER (ORDER BY a RANGE BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW)) / (count(*) OVER ()) FROM t ORDER BY a"})
for op, q in [("~", "SELECT bitNot(5)"), ("&", "SELECT bitAnd(6, 3)"), ("|", "SELECT bitOr(6, 3)"), ("<<", "SELECT bitShiftLeft(1, 4)"), (">>", "SELECT bitShiftRight(256, 2)"),
              ("- (unary)", "SELECT -(3), -toInt32('4')")]:
    rule(f"operator:{op}", ch={"query": q})
rule("operator:COLLATE", ch=none("ClickHouse compares strings by bytes: COLLATE orders an ORDER BY, it doesn't change a comparison"))
rule("expression:boolean expression", ch=none("ClickHouse: " + NO_TRUTH))
rule("keyword:CONSTRAINT", ch={"setup": ["CREATE TABLE kc(a Int32, CONSTRAINT pos CHECK a > 0) ENGINE = MergeTree ORDER BY a"], "query": "INSERT INTO kc VALUES (1); SELECT a FROM kc"})
NO_FK = "ClickHouse has no foreign keys"
for k in ["NO", "ACTION", "RESTRICT"]:
    rule(f"keyword:{k}", ch=none(NO_FK))
rule("parameter:?NNN", duck={"query": "SELECT $1 + 1"})
rule("syntax:ordering-term#1", ch={"query": "SELECT b FROM t ORDER BY lower(b) DESC NULLS FIRST"})
rule("syntax:update-stmt#1", ch={"query": "ALTER TABLE t UPDATE b = upper(b), c = c + 1 WHERE a < 3 SETTINGS mutations_sync = 1"})
rule("syntax:update-stmt#3", ch={"query": "ALTER TABLE t UPDATE b = 'p', c = 9.5 WHERE a = 2 SETTINGS mutations_sync = 1"})
rule("syntax:update-stmt#4", ch=none("ClickHouse's ALTER TABLE ... UPDATE can't join another table"))
rule("syntax:column-name-list", ch={"query": "ALTER TABLE t UPDATE a = 1, b = 'z' WHERE a = 4 SETTINGS mutations_sync = 1"})
rule("syntax:create-table-stmt#1", ch={"query": "CREATE TABLE IF NOT EXISTS ct (a Int32, b String DEFAULT 'x', c Float64, CONSTRAINT c_nonneg CHECK c >= 0) ENGINE = MergeTree ORDER BY a"})
rule("syntax:column-constraint#2", ch=none("ClickHouse has no UNIQUE constraint"))
rule("syntax:column-constraint#3", ch=none(NO_FK))
rule("syntax:table-constraint#1", ch={"query": "CREATE TABLE tc(a Int32, b Int32, CONSTRAINT ab CHECK a < b) ENGINE = MergeTree PRIMARY KEY (a, b) ORDER BY (a, b); INSERT INTO tc VALUES (1, 2); SELECT count(*) FROM tc"})
rule("syntax:table-constraint#2", ch=none(NO_FK))
rule("syntax:foreign-key-clause", ch=none(NO_FK))
rule("syntax:create-index-stmt#1", ch=none("ClickHouse has no unique indexes: its indexes skip data, they don't constrain it"))
rule("syntax:create-index-stmt#2", ch=none("ClickHouse has no partial indexes"))
rule("syntax:create-index-stmt#3", ch={"query": "CREATE INDEX ex ON t(lower(b)) TYPE bloom_filter GRANULARITY 1"})
rule("syntax:create-index-stmt#4", ch=none("ClickHouse's indexes take no collation"))
rule("syntax:indexed-column", ch=none("ClickHouse's indexes take no collation"))
rule("syntax:alter-table-stmt#1", ch={"query": "RENAME TABLE t TO t9"})
rule("syntax:drop-index-stmt", ch={"setup_tail": ["CREATE INDEX ti ON t(a) TYPE minmax GRANULARITY 1"], "query": "DROP INDEX IF EXISTS ti ON t"})
NO_TX = "ClickHouse's transactions are experimental and off by default"
for pid in ["syntax:commit-stmt#1", "syntax:commit-stmt#2", "syntax:rollback-stmt", "syntax:release-stmt", "syntax:savepoint-stmt"]:
    rule(pid, ch=none(NO_TX))
rule("syntax:type-name#1", ch={"query": "CREATE TABLE tn(a String, b Decimal(10, 2), c Float64) ENGINE = MergeTree ORDER BY tuple(); INSERT INTO tn VALUES ('x', 1.25, 2.5); SELECT a, b, c FROM tn"})

# ---------------------------------------------------------------- the datatype rules where only SQLite's type system says them
TYPE_NAMES = "{} takes only its own type names: SQLite's affinity from the letters of any declared type has no counterpart there"
rule("rule.affinity:the rules apply in order", pg=none(TYPE_NAMES.format("PostgreSQL")), my=none(TYPE_NAMES.format("MySQL")),
     duck=none(TYPE_NAMES.format("DuckDB")), ch=none(TYPE_NAMES.format("ClickHouse")))
ONE_TYPE = "ClickHouse compares and stores values of one type at a time: {}"
rule("rule.comparison:numbers before text and blobs", ch=none(ONE_TYPE.format("a number against text is an error, not an ordering")))
rule("rule.comparison:text before blobs", ch={"query": "SELECT 'zzz' < unhex('00')"})
rule("rule.comparison:blobs compare by memcmp", ch={"query": "SELECT unhex('0102') < unhex('0201'), unhex('01') < unhex('0100')"})
rule("rule.comparison:otherwise compared as is", ch={"query": "SELECT '10' = 10, unhex('31') = '1'"})
rule("rule.sort:ORDER BY orders by storage class", ch=none(ONE_TYPE.format("values of different classes can't share a column")))

# ---------------------------------------------------------------- PRAGMAs: SQLite's own settings and catalog
PRAGMA_WHY = {
    PG: "PostgreSQL has no PRAGMA: server settings are SHOW and SET (postgresql.conf), the catalog is information_schema and pg_catalog",
    MY: "MySQL has no PRAGMA: server settings are SHOW VARIABLES and SET, the catalog is information_schema",
    CH: "ClickHouse has no PRAGMA: settings are in system.settings and SET, the catalog in system tables",
}
CATALOG = {
    "table_info": {PG: "SELECT ordinal_position - 1 AS cid, column_name AS name, upper(data_type) AS type, (is_nullable = 'NO')::int AS notnull, column_default AS dflt_value, 0 AS pk FROM information_schema.columns WHERE table_name = 't' AND table_schema = current_schema()",
                   MY: "SELECT ordinal_position - 1 AS cid, column_name AS name, upper(data_type) AS type, (is_nullable = 'NO') AS notnull, column_default AS dflt_value, 0 AS pk FROM information_schema.columns WHERE table_name = 't' AND table_schema = database()",
                   DUCK: "PRAGMA table_info('t')",
                   CH: "SELECT position - 1 AS cid, name, type, 0 AS notnull, default_expression AS dflt_value, is_in_primary_key AS pk FROM system.columns WHERE table = 't' AND database = currentDatabase()"},
    "table_list": {PG: "SELECT table_schema, table_name, table_type FROM information_schema.tables WHERE table_schema = current_schema()",
                   MY: "SELECT table_schema, table_name, table_type FROM information_schema.tables WHERE table_schema = database()",
                   DUCK: "SELECT schema_name, table_name, 'table' FROM duckdb_tables()",
                   CH: "SELECT database, name, engine FROM system.tables WHERE database = currentDatabase()"},
    "database_list": {PG: "SELECT 0, current_database(), ''", MY: "SELECT 0, database(), ''", DUCK: "PRAGMA database_list", CH: "SELECT 0, currentDatabase(), ''"},
    "index_list": {PG: "SELECT indexname FROM pg_indexes WHERE tablename = 't' AND schemaname = current_schema()",
                   MY: "SELECT DISTINCT index_name FROM information_schema.statistics WHERE table_name = 't' AND table_schema = database()",
                   DUCK: "SELECT index_name FROM duckdb_indexes() WHERE table_name = 't'",
                   CH: "SELECT name FROM system.data_skipping_indices WHERE table = 't' AND database = currentDatabase()"},
    "foreign_key_list": {PG: "SELECT constraint_name FROM information_schema.table_constraints WHERE table_name = 't' AND constraint_type = 'FOREIGN KEY'",
                         MY: "SELECT constraint_name FROM information_schema.table_constraints WHERE table_name = 't' AND constraint_type = 'FOREIGN KEY' AND table_schema = database()",
                         DUCK: "SELECT constraint_text FROM duckdb_constraints() WHERE table_name = 't' AND constraint_type = 'FOREIGN KEY'"},
    "function_list": {PG: "SELECT DISTINCT proname FROM pg_proc LIMIT 0", MY: "SELECT routine_name FROM information_schema.routines LIMIT 0",
                      DUCK: "SELECT function_name FROM duckdb_functions() LIMIT 0", CH: "SELECT name FROM system.functions LIMIT 0"},
    "collation_list": {PG: "SELECT collname FROM pg_collation LIMIT 0", MY: "SELECT collation_name FROM information_schema.collations LIMIT 0",
                       DUCK: "SELECT collname FROM pragma_collations() LIMIT 0", CH: "SELECT name FROM system.collations LIMIT 0"},
    "integrity_check": {MY: "CHECK TABLE t", CH: "CHECK TABLE t"},
    "quick_check": {MY: "CHECK TABLE t QUICK", CH: "CHECK TABLE t"},
    "optimize": {PG: "ANALYZE", MY: "ANALYZE TABLE t", DUCK: "ANALYZE"},
    "wal_checkpoint": {PG: "CHECKPOINT", DUCK: "CHECKPOINT"},
    "page_size": {PG: "SHOW block_size", DUCK: "SELECT current_setting('default_block_size') IS NOT NULL"},
    "encoding": {PG: "SHOW server_encoding", MY: "SELECT @@character_set_database"},
    "case_sensitive_like": {},
}


def add_pragmas(probes):
    for p in probes:
        if not p["id"].startswith("pragma:"):
            continue
        name = p["id"].split(":", 1)[1].split("#")[0]
        spelled = CATALOG.get(name, {})
        for d in (PG, MY, DUCK, CH):
            if p["id"] in RULES and d in RULES[p["id"]]:
                continue
            if d in spelled:
                RULES.setdefault(p["id"], {})[d] = {"query": spelled[d]}
                if d == CH and name.startswith("index_"):
                    # ClickHouse's indexes skip data and must name a type.
                    RULES[p["id"]][d]["setup_tail"] = ["CREATE INDEX ti ON t(a) TYPE minmax GRANULARITY 1"]
            elif d == DUCK:
                # DuckDB has a PRAGMA statement of its own: sqlglot's translation is tried, and
                # what DuckDB lacks shows as its refusal.
                continue
            else:
                RULES.setdefault(p["id"], {})[d] = none(PRAGMA_WHY[d] + f"; nothing there answers PRAGMA {name}")


def main():
    probes = json.load(open(sys.argv[2]))["probes"] if len(sys.argv) > 2 else []
    add_pragmas(probes)
    add_collations(probes)
    json.dump(RULES, open(sys.argv[1], "w"), indent=1, sort_keys=True)
    print(f"{sum(len(v) for v in RULES.values())} spellings for {len(RULES)} probes")


main()
