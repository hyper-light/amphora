# tlp Panic: selected partial index predicate must be implied by query

seed 11, state 361

## Statements

```sql
CREATE TABLE t0(c0 INTEGER UNIQUE, c1 INTEGER COLLATE RTRIM PRIMARY KEY, c2 BOOLEAN COLLATE RTRIM, c3 INT NOT NULL);
```

## Queries

```sql
```

## What differed

```
selected partial index predicate must be implied by query
```
