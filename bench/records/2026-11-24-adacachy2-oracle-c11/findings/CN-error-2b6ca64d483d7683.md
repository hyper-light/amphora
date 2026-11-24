# tlp Error: the partitions' UNION errors on Amphora, not on SQLite: Parse error: row value misused

seed 8003, state 263

## Statements

```sql
CREATE TABLE t0(c0 DECIMAL(10,2), c1 NUMERIC COLLATE BINARY, c2  COLLATE RTRIM, c3 DOUBLE PRIMARY KEY);
CREATE TABLE t1(c0 BLOB PRIMARY KEY, c1 INTEGER) WITHOUT ROWID;
CREATE TABLE lg(c0, c1, c2);
CREATE VIEW v0(c0, c1, c2) AS SELECT DISTINCT quote((b98x1.c1 > b98x1.c1)), quote((EXISTS (SELECT 1 FROM t1 AS s898 WHERE (s898.c1 IS DISTINCT FROM NULL)))), CAST((CAST(b98x0.c0 AS TEXT) & (CASE WHEN (b98x0.c2 <= x'00') THEN b98x1.c0 END)) AS INTEGER) FROM t0 AS b98x0 LEFT JOIN t1 AS b98x1 ON (0.5 NOT BETWEEN ('' - b98x1.c1) AND 9223372036854775807);
```

## Queries

```sql
SELECT DISTINCT jsonb_remove(' ', lower(a1.c2), (a1.c0 BETWEEN 'x' AND 1e308)) FROM t0 AS a0, v0 AS a1 WHERE (3 ISNULL);
SELECT DISTINCT jsonb_remove(' ', lower(a1.c2), (a1.c0 BETWEEN 'x' AND 1e308)) FROM t0 AS a0, v0 AS a1 WHERE ((3 ISNULL) AND ((((x'' > a0.c0), a0.c0, a0.c2) < (SELECT max(quote(s857.c2)), count(quote(s857.c0)), max(quote(s857.c1)) FROM lg AS s857)) OR (a0.c2 NOT LIKE 'a%'))) UNION SELECT DISTINCT jsonb_remove(' ', lower(a1.c2), (a1.c0 BETWEEN 'x' AND 1e308)) FROM t0 AS a0, v0 AS a1 WHERE ((3 ISNULL) AND (NOT ((((x'' > a0.c0), a0.c0, a0.c2) < (SELECT max(quote(s857.c2)), count(quote(s857.c0)), max(quote(s857.c1)) FROM lg AS s857)) OR (a0.c2 NOT LIKE 'a%')))) UNION SELECT DISTINCT jsonb_remove(' ', lower(a1.c2), (a1.c0 BETWEEN 'x' AND 1e308)) FROM t0 AS a0, v0 AS a1 WHERE ((3 ISNULL) AND (((((x'' > a0.c0), a0.c0, a0.c2) < (SELECT max(quote(s857.c2)), count(quote(s857.c0)), max(quote(s857.c1)) FROM lg AS s857)) OR (a0.c2 NOT LIKE 'a%')) ISNULL));
```

## What differed

```
the partitions' UNION errors on Amphora, not on SQLite: Parse error: row value misused
```
