# The SQLite language against every engine (G2 step 1)

Every entry of `features.json` (570, from SQLite's documentation sqlite-doc-3540100), probed on each engine with `probes.json` (510 probes) and compared with SQLite's own answers. Built by `bench/corpus/matrix.py`; each entry's per-engine spelling and every disagreement are in `matrix.json`. The probes' answers, engine by engine: `bench/records/2026-10-24-adacachy-corpus-probes/`.

## The inventory

`inventory.py` reads SQLite's documentation; `check_inventory.py` holds the result against every list the documentation keeps of its own language and against SQLite's own catalog (pragma_function_list, pragma_pragma_list): nothing documented may be missing.

| kind | entries |
|---|---:|
| syntax | 71 |
| function.core | 51 |
| function.aggregate | 12 |
| function.window | 11 |
| function.datetime | 7 |
| function.math | 29 |
| function.json | 34 |
| datetime.modifier | 19 |
| pragma | 74 |
| operator | 51 |
| parameter | 5 |
| expression | 4 |
| affinity | 5 |
| collation | 3 |
| storage_class | 5 |
| rule.affinity | 6 |
| rule.expression-affinity | 8 |
| rule.comparison | 11 |
| rule.sort | 3 |
| rule.collation | 7 |
| keyword | 147 |
| word | 7 |
| all | 570 |

```
lang.html topics                         40 of   40  100%
syntaxdiagrams.html productions          71 of   71  100%
lang_corefunc.html index                 51 of   51  100%
lang_aggfunc.html index                  12 of   12  100%
lang_mathfunc.html index                 29 of   29  100%
lang_datefunc.html signatures             7 of    7  100%
json1.html headings                      28 of   28  100%
windowfunctions.html built-ins           11 of   11  100%
pragma.html list                         74 of   74  100%
lang_keywords.html list                 147 of  147  100%
syntax diagrams' words                  154 of  154  100%
lang_expr.html operator table            38 of   38  100%
lang_expr.html NOT forms                  6 of    6  100%
SQLite's function_list (build 3.54.0)   143 of  143  100%
SQLite's pragma_list (build 3.54.0)      66 of   66  100%
nothing documented missing: 570 entries
```

## Every engine against SQLite

| engine | version | as SQLite (entries) |
|---|---|---:|
| SQLite (reference) | SQLite 3.54.0 (Python's sqlite3) | - |
| Amphora | Amphora e8d9a31 | 463 of 570 (81%) |
| Turso | Turso 0.9.0-pre.1 at 3e09a82 | 466 of 570 (82%) |
| DuckDB | DuckDB 2.0.0 (Python module) | 292 of 570 (51%) |
| PostgreSQL | PostgreSQL 17.11 (Debian 17.11-1.pgdg13+2) | 309 of 570 (54%) |
| OrioleDB | PostgreSQL 17.11 OrioleDB beta 19 | 309 of 570 (54%) |
| CedarDB | PostgreSQL 16.3 compatible CedarDB v2026-10-08 | 259 of 570 (45%) |
| MySQL | MySQL 8.4.13 | 278 of 570 (49%) |
| ClickHouse | ClickHouse 26.8.34.13 | 252 of 570 (44%) |

**Entries answering as SQLite does, by kind** (a keyword counts once every statement using it answers as SQLite does):

| kind | entries | Amphora | Turso | DuckDB | PostgreSQL | OrioleDB | CedarDB | MySQL | ClickHouse |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| syntax | 71 | 61 | 61 | 41 | 53 | 53 | 45 | 45 | 37 |
| function.core | 51 | 47 | 47 | 31 | 33 | 33 | 31 | 34 | 32 |
| function.aggregate | 12 | 6 | 6 | 12 | 12 | 12 | 12 | 8 | 11 |
| function.window | 11 | 11 | 11 | 11 | 11 | 11 | 11 | 11 | 11 |
| function.datetime | 7 | 7 | 7 | 4 | 5 | 5 | 3 | 5 | 5 |
| function.math | 29 | 29 | 29 | 29 | 29 | 29 | 18 | 28 | 27 |
| function.json | 34 | 30 | 30 | 17 | 5 | 5 | 2 | 4 | 14 |
| datetime.modifier | 19 | 19 | 19 | 13 | 17 | 17 | 17 | 13 | 13 |
| pragma | 74 | 42 | 45 | 3 | 4 | 4 | 4 | 2 | 1 |
| operator | 51 | 49 | 49 | 45 | 43 | 43 | 40 | 45 | 44 |
| parameter | 5 | 5 | 5 | 5 | 0 | 0 | 0 | 0 | 0 |
| expression | 4 | 4 | 4 | 3 | 3 | 3 | 3 | 4 | 2 |
| affinity | 5 | 5 | 5 | 0 | 0 | 0 | 0 | 0 | 0 |
| collation | 3 | 3 | 3 | 1 | 2 | 2 | 0 | 2 | 0 |
| storage_class | 5 | 5 | 5 | 0 | 0 | 0 | 0 | 0 | 0 |
| rule.affinity | 6 | 6 | 6 | 0 | 0 | 0 | 0 | 0 | 0 |
| rule.expression-affinity | 8 | 8 | 8 | 3 | 2 | 2 | 3 | 3 | 3 |
| rule.comparison | 11 | 11 | 11 | 5 | 4 | 4 | 4 | 5 | 5 |
| rule.sort | 3 | 3 | 3 | 1 | 0 | 0 | 0 | 0 | 0 |
| rule.collation | 7 | 6 | 6 | 2 | 3 | 3 | 2 | 1 | 2 |
| keyword | 147 | 102 | 102 | 63 | 79 | 79 | 61 | 64 | 41 |
| word | 7 | 4 | 4 | 3 | 4 | 4 | 3 | 4 | 4 |

**Every entry's status, counted** (an entry's worst-represented outcome when its probes split):

| status | Amphora | Turso | DuckDB | PostgreSQL | OrioleDB | CedarDB | MySQL | ClickHouse |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| same | 463 | 466 | 292 | 309 | 309 | 259 | 278 | 252 |
| partial | 40 | 40 | 67 | 65 | 65 | 73 | 73 | 74 |
| same values, other form | 0 | 0 | 1 | 19 | 19 | 8 | 27 | 1 |
| differs | 33 | 30 | 58 | 43 | 43 | 41 | 53 | 55 |
| error | 29 | 29 | 85 | 27 | 27 | 79 | 22 | 17 |
| no equivalent | 0 | 0 | 65 | 100 | 100 | 103 | 109 | 166 |
| not applicable | 0 | 0 | 0 | 5 | 5 | 5 | 5 | 5 |
| no reference | 5 | 5 | 2 | 2 | 2 | 2 | 3 | 0 |

**How each engine's probes were spelled** (non-keyword entries):

| spelling | Amphora | Turso | DuckDB | PostgreSQL | OrioleDB | CedarDB | MySQL | ClickHouse |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| SQLite's own | 423 | 423 | 0 | 0 | 0 | 0 | 0 | 0 |
| hand-written | 0 | 0 | 72 | 116 | 116 | 116 | 115 | 108 |
| its driver's parameter syntax | 0 | 0 | 0 | 5 | 5 | 5 | 5 | 5 |
| no equivalent | 0 | 0 | 46 | 91 | 91 | 91 | 98 | 131 |
| sqlglot | 0 | 0 | 305 | 211 | 211 | 211 | 205 | 179 |

## Where Amphora doesn't answer as SQLite does (57 entries)

- `syntax:aggregate-function-invocation` (partial): 2 error: Parse error: ORDER BY clause is not supported yet in aggregate functions
- `syntax:attach-stmt` (error): error: Parse error: ATTACH is an experimental feature. Enable with --experimental-attach flag
- `syntax:column-def` (partial): 3 error: Parse error: Generated columns require --experimental-generated-columns flag; 4 error: Parse error: Generated columns require --experimental-generated-columns flag
- `syntax:create-trigger-stmt` (partial): 4 error: Parse error: INSTEAD OF triggers are not supported yet
- `syntax:create-virtual-table-stmt` (error): 1 error: Parse error: no such module: fts5; 2 error: Parse error: no such module: rtree
- `syntax:detach-stmt` (error): error: Parse error: ATTACH is an experimental feature. Enable with --experimental-attach flag
- `syntax:table-options` (partial): 1 error: Parse error: WITHOUT ROWID tables are an experimental feature. Enable with --experimental-without-rowid flag; 3 error: Parse error: WITHOUT ROWID tables are an experimental feature. Enable with --experimental-without-rowid flag
- `syntax:vacuum-stmt` (error): error: Parse error: VACUUM is an experimental feature. Enable with --experimental-vacuum flag
- `function.core:sqlite_compileoption_get` (error): error: Parse error: no such function: sqlite_compileoption_get
- `function.core:sqlite_compileoption_used` (error): error: Parse error: no such function: sqlite_compileoption_used
- `function.aggregate:group_concat` (error): error: Parse error: ORDER BY clause is not supported yet in aggregate functions
- `function.aggregate:median` (error): error: Parse error: no such function: median
- `function.aggregate:percentile` (error): error: Parse error: no such function: percentile
- `function.aggregate:percentile_cont` (error): error: Parse error: no such function: percentile_cont
- `function.aggregate:percentile_disc` (error): error: Parse error: no such function: percentile_disc
- `function.aggregate:string_agg` (error): error: Parse error: ORDER BY clause is not supported yet in aggregate functions
- `function.json:json_array_insert` (error): error: Parse error: no such function: json_array_insert
- `function.json:jsonb_array_insert` (error): error: Parse error: no such function: jsonb_array_insert
- `function.json:jsonb_each` (error): error: Parse error: no such table: jsonb_each
- `function.json:jsonb_tree` (error): error: Parse error: no such table: jsonb_tree
- `pragma:analysis_limit` (differs): differs: SQLite [[0]], here []
- `pragma:automatic_index` (differs): differs: SQLite [[1]], here []
- `pragma:cache_size` (differs): differs: SQLite [[-2000]], here [[1024]]
- `pragma:cache_spill` (differs): differs: SQLite [[483]], here [[1]]
- `pragma:cell_size_check` (differs): differs: SQLite [[0]], here []
- `pragma:checkpoint_fullfsync` (differs): differs: SQLite [[0]], here []
- `pragma:collation_list` (differs): differs: no rows, where SQLite answers
- `pragma:compile_options` (differs): differs: no rows, where SQLite answers
- `pragma:data_version` (differs): differs: SQLite 1 rows of 1, here 0 rows of 0
- `pragma:default_cache_size` (differs): differs: SQLite [[-2000]], here []
- `pragma:defer_foreign_keys` (differs): differs: SQLite [[0]], here []
- `pragma:empty_result_callbacks` (differs): differs: SQLite [[0]], here []
- `pragma:fullfsync` (differs): differs: SQLite [[0]], here []
- `pragma:hard_heap_limit` (differs): differs: SQLite [[0]], here []
- `pragma:index_xinfo` (differs): differs: SQLite [[0, 0, "a", 0, "BINARY", 1], [1, -1, null, 0, "BINARY", 0]], here [[0, 0, "a", 0, "BINARY", 1], [1, -1, "", 0, "BINARY", 0]]
- `pragma:journal_mode` (differs): differs: SQLite [["delete"]], here [["wal"]]
- `pragma:journal_size_limit` (differs): differs: SQLite [[-1]], here []
- `pragma:legacy_alter_table` (differs): differs: SQLite [[0]], here []
- `pragma:locking_mode` (differs): differs: SQLite [["normal"]], here [["exclusive"]]
- `pragma:mmap_size` (differs): differs: SQLite [[0]], here []
- `pragma:module_list` (differs): differs: no rows, where SQLite answers
- `pragma:read_uncommitted` (differs): differs: SQLite [[0]], here []
- `pragma:recursive_triggers` (differs): differs: SQLite [[0]], here []
- `pragma:reverse_unordered_selects` (differs): differs: SQLite [[0]], here []
- `pragma:secure_delete` (differs): differs: SQLite [[1]], here []
- `pragma:soft_heap_limit` (differs): differs: SQLite [[0]], here []
- `pragma:synchronous` (differs): differs: SQLite [[2]], here [[1]]
- `pragma:threads` (differs): differs: SQLite [[0]], here []
- `pragma:trusted_schema` (differs): differs: SQLite [[1]], here []
- `pragma:wal_autocheckpoint` (differs): differs: SQLite [[1000]], here [[0]]
- `pragma:wal_checkpoint` (differs): differs: SQLite [[0, -1, -1]], here [[0, 6, 6]]
- `pragma:writable_schema` (differs): differs: SQLite [[0]], here []
- `operator:MATCH` (error): error: Parse error: no such module: fts5
- `rule.collation:IN uses the left operand's collation` (differs): differs: SQLite [[1, 0]], here [[1, 1]]
- `word:EXPRESSIONS` (error): error: Parse error: unable to identify the object to be reindexed
- `word:ROWID` (error): error: Parse error: WITHOUT ROWID tables are an experimental feature. Enable with --experimental-without-rowid flag
- `word:STORED` (error): error: Parse error: Generated columns require --experimental-generated-columns flag

## Where Turso doesn't answer as SQLite does (54 entries)

- `syntax:aggregate-function-invocation` (partial): 2 error: Parse error: ORDER BY clause is not supported yet in aggregate functions
- `syntax:attach-stmt` (error): error: Parse error: ATTACH is an experimental feature. Enable with --experimental-attach flag
- `syntax:column-def` (partial): 3 error: Parse error: Generated columns require --experimental-generated-columns flag; 4 error: Parse error: Generated columns require --experimental-generated-columns flag
- `syntax:create-trigger-stmt` (partial): 4 error: Parse error: INSTEAD OF triggers are not supported yet
- `syntax:create-virtual-table-stmt` (error): 1 error: Parse error: no such module: fts5; 2 error: Parse error: no such module: rtree
- `syntax:detach-stmt` (error): error: Parse error: ATTACH is an experimental feature. Enable with --experimental-attach flag
- `syntax:table-options` (partial): 1 error: Parse error: WITHOUT ROWID tables are an experimental feature. Enable with --experimental-without-rowid flag; 3 error: Parse error: WITHOUT ROWID tables are an experimental feature. Enable with --experimental-without-rowid flag
- `syntax:vacuum-stmt` (error): error: Parse error: VACUUM is an experimental feature. Enable with --experimental-vacuum flag
- `function.core:sqlite_compileoption_get` (error): error: Parse error: no such function: sqlite_compileoption_get
- `function.core:sqlite_compileoption_used` (error): error: Parse error: no such function: sqlite_compileoption_used
- `function.aggregate:group_concat` (error): error: Parse error: ORDER BY clause is not supported yet in aggregate functions
- `function.aggregate:median` (error): error: Parse error: no such function: median
- `function.aggregate:percentile` (error): error: Parse error: no such function: percentile
- `function.aggregate:percentile_cont` (error): error: Parse error: no such function: percentile_cont
- `function.aggregate:percentile_disc` (error): error: Parse error: no such function: percentile_disc
- `function.aggregate:string_agg` (error): error: Parse error: ORDER BY clause is not supported yet in aggregate functions
- `function.json:json_array_insert` (error): error: Parse error: no such function: json_array_insert
- `function.json:jsonb_array_insert` (error): error: Parse error: no such function: jsonb_array_insert
- `function.json:jsonb_each` (error): error: Parse error: no such table: jsonb_each
- `function.json:jsonb_tree` (error): error: Parse error: no such table: jsonb_tree
- `pragma:analysis_limit` (differs): differs: SQLite [[0]], here []
- `pragma:automatic_index` (differs): differs: SQLite [[1]], here []
- `pragma:cache_spill` (differs): differs: SQLite [[483]], here [[1]]
- `pragma:cell_size_check` (differs): differs: SQLite [[0]], here []
- `pragma:checkpoint_fullfsync` (differs): differs: SQLite [[0]], here []
- `pragma:collation_list` (differs): differs: no rows, where SQLite answers
- `pragma:compile_options` (differs): differs: no rows, where SQLite answers
- `pragma:data_version` (differs): differs: SQLite 1 rows of 1, here 0 rows of 0
- `pragma:default_cache_size` (differs): differs: SQLite [[-2000]], here []
- `pragma:defer_foreign_keys` (differs): differs: SQLite [[0]], here []
- `pragma:empty_result_callbacks` (differs): differs: SQLite [[0]], here []
- `pragma:fullfsync` (differs): differs: SQLite [[0]], here []
- `pragma:hard_heap_limit` (differs): differs: SQLite [[0]], here []
- `pragma:index_xinfo` (differs): differs: SQLite [[0, 0, "a", 0, "BINARY", 1], [1, -1, null, 0, "BINARY", 0]], here [[0, 0, "a", 0, "BINARY", 1], [1, -1, "", 0, "BINARY", 0]]
- `pragma:journal_mode` (differs): differs: SQLite [["delete"]], here [["wal"]]
- `pragma:journal_size_limit` (differs): differs: SQLite [[-1]], here []
- `pragma:legacy_alter_table` (differs): differs: SQLite [[0]], here []
- `pragma:locking_mode` (differs): differs: SQLite [["normal"]], here [["exclusive"]]
- `pragma:mmap_size` (differs): differs: SQLite [[0]], here []
- `pragma:module_list` (differs): differs: no rows, where SQLite answers
- `pragma:read_uncommitted` (differs): differs: SQLite [[0]], here []
- `pragma:recursive_triggers` (differs): differs: SQLite [[0]], here []
- `pragma:reverse_unordered_selects` (differs): differs: SQLite [[0]], here []
- `pragma:secure_delete` (differs): differs: SQLite [[1]], here []
- `pragma:soft_heap_limit` (differs): differs: SQLite [[0]], here []
- `pragma:threads` (differs): differs: SQLite [[0]], here []
- `pragma:trusted_schema` (differs): differs: SQLite [[1]], here []
- `pragma:wal_checkpoint` (differs): differs: SQLite [[0, -1, -1]], here [[0, 6, 6]]
- `pragma:writable_schema` (differs): differs: SQLite [[0]], here []
- `operator:MATCH` (error): error: Parse error: no such module: fts5
- `rule.collation:IN uses the left operand's collation` (differs): differs: SQLite [[1, 0]], here [[1, 1]]
- `word:EXPRESSIONS` (error): error: Parse error: unable to identify the object to be reindexed
- `word:ROWID` (error): error: Parse error: WITHOUT ROWID tables are an experimental feature. Enable with --experimental-without-rowid flag
- `word:STORED` (error): error: Parse error: Generated columns require --experimental-generated-columns flag

## Probes that crashed a server

- CedarDB: `operator:OR`
