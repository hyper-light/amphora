# diff Wrong: upsert_set_twice

plain

## Statements

```sql
CREATE TABLE t(k INTEGER PRIMARY KEY, v TEXT);
INSERT INTO t VALUES (1, 'a');
INSERT INTO t VALUES (1, 'b') ON CONFLICT(k) DO UPDATE SET v = 'first', v = 'second';
```

## Queries

```sql
SELECT * FROM t;
```

## What differed

```
```
