# diff Wrong: row_compare_call

plain

## Statements

```sql
CREATE TABLE t(a TEXT, b TEXT, c INT);
INSERT INTO t VALUES ('x', 'y', 1), ('z', 'w', 2);
```

## Queries

```sql
SELECT c FROM t WHERE (a, b) > (lower('X'), json_extract('["a"]', '$[0]'));
SELECT c FROM t WHERE (a, b) BETWEEN ('a', 'a') AND ('y', b);
SELECT c FROM t WHERE (a, 'k') BETWEEN (a, 'a') AND (a, b);
SELECT c FROM t WHERE ('k', a) < (lower(b), upper(a));
SELECT c FROM t WHERE (json_extract('[1]', '$[0]'), a) = (1, 'x');
```

## What differed

```
```
