# eet Wrong: an equivalent form gives other rows

seed 12, state 9165

## Statements

```sql
CREATE TABLE t1(c0 TEXT COLLATE BINARY);
INSERT INTO t1 VALUES (' a');
CREATE INDEX i0 ON t1(c0 DESC);
```

## Queries

```sql
SELECT (a0.c0 >> a0.c0), json_extract('"x"', '$.a'), count(*) FROM t1 AS a0 WHERE ((NULL < a0.c0) OR (a0.c0 GLOB '[a-z]*')) GROUP BY (a0.c0 >> a0.c0), json_extract('"x"', '$.a');
SELECT (a0.c0 >> a0.c0), json_extract('"x"', '$.a'), count(*) FROM t1 AS a0 WHERE ((NULL < a0.c0) OR (((NULL OR (NOT NULL)) OR (NULL ISNULL)) AND (a0.c0 GLOB (CASE WHEN (('1' OR (NOT '1')) OR ('1' ISNULL)) THEN '[a-z]*' ELSE NULL END)))) GROUP BY (a0.c0 >> a0.c0), json_extract('"x"', '$.a');
```

## What differed

```
an equivalent form gives other rows
  expected: 1 rows
    0, NULL, 1
  Amphora:  0 rows
```
