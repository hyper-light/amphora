# PostgreSQL 19.0's parser sources and regression SQL, vendored

What Amphora's PostgreSQL front end is built and checked from (docs/design/postgres.md section 3).

- **Upstream:** https://github.com/postgres/postgres, tag `REL_19_0`, commit
  `5c839a2a08b9b87a349be97371fac6c5a2873b10`, under the PostgreSQL Licence (`COPYRIGHT`).
- **What is here:** the grammar and scanner (`src/backend/parser/gram.y`, `scan.l`), the lookahead
  filter and scanner support they need (`parser.c`, `scansup.c`), the keyword list
  (`src/include/parser/kwlist.h`), the parse nodes the grammar's actions build
  (`src/include/nodes/parsenodes.h`, `primnodes.h`), the SQLSTATEs (`src/backend/utils/errcodes.txt`),
  and the regression suite's SQL (`src/test/regress/sql`), the corpus the parser oracle
  (crates/pg-oracle) gives verdicts on.
- **How it's used:** read as data. Nothing here is compiled into Amphora; the oracle that runs
  PostgreSQL's own parser is built apart by scripts/postgres-oracle-extract.sh.

## Changes from upstream

None.
