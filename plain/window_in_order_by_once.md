# diff Wrong: window_in_order_by_once

plain

## Statements

```sql
CREATE TABLE t(id INTEGER PRIMARY KEY, v INT);
INSERT INTO t VALUES (1, 5), (2, 3);
```

## Queries

```sql
SELECT id FROM t ORDER BY abs(first_value(v + 1) OVER (ORDER BY id));
SELECT id, lag(v) OVER (ORDER BY id) FROM t ORDER BY typeof(lag(v + 1) OVER (ORDER BY id)), id;
```

## What differed

```
```
