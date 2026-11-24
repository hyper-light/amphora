# eet Error: the equivalent form errors on Amphora, not on SQLite: Parse error: insufficient registers allocated for expression vector write (start=24, count=2, next_free=25)

seed 27005, state 19

## Statements

```sql
CREATE TABLE t1(c0 VARCHAR(10));
CREATE TABLE lg(c0, c1, c2);
```

## Queries

```sql
SELECT 3 FROM (SELECT char((0.1 == b81x0.c2)) AS c0, CAST((b81x0.c1 IS NOT NULL) AS INTEGER) AS c1, instr(NULL, trim('1e3', b81x0.c2)) AS c2 FROM lg AS b81x0 WHERE ((b81x0.c0 BETWEEN 3.0 AND 1e308) NOTNULL)) AS a0 CROSS JOIN t1 AS a1 USING (c0);
SELECT (CASE WHEN ((((a1.c0, (CASE WHEN (((a0.c1 IS NOT c0) AND (NOT (a0.c1 IS NOT c0))) AND ((a0.c1 IS NOT c0) NOTNULL)) THEN 0 ELSE x'41' END)) BETWEEN (a1.c0, x'') AND (-1, c0)) OR (NOT ((a1.c0, x'41') BETWEEN (a1.c0, x'') AND (-1, c0)))) OR ((((c0 >= a0.c1) OR (NOT (c0 >= a0.c1))) OR ((c0 >= a0.c1) ISNULL)) AND (((a1.c0, x'41') BETWEEN (a1.c0, x'') AND (-1, c0)) ISNULL))) THEN 3 ELSE 10 END) FROM (SELECT char((0.1 == b81x0.c2)) AS c0, CAST((b81x0.c1 IS NOT NULL) AS INTEGER) AS c1, instr(NULL, trim('1e3', b81x0.c2)) AS c2 FROM lg AS b81x0 WHERE ((b81x0.c0 BETWEEN 3.0 AND 1e308) NOTNULL)) AS a0 CROSS JOIN t1 AS a1 USING (c0);
```

## What differed

```
the equivalent form errors on Amphora, not on SQLite: Parse error: insufficient registers allocated for expression vector write (start=24, count=2, next_free=25)
```
