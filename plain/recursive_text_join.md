# diff Wrong: recursive_text_join

plain

## Statements

```sql
CREATE TABLE edge(src TEXT, dst TEXT);
INSERT INTO edge VALUES ('1', '2'), ('2', '3');
```

## Queries

```sql
WITH RECURSIVE r(n, d) AS (SELECT 1, 0 UNION ALL SELECT edge.dst, r.d + 1 FROM r JOIN edge ON edge.src = r.n WHERE r.d < 5) SELECT n, d FROM r;
WITH r(n) AS (SELECT 1) SELECT edge.dst FROM r JOIN edge ON edge.src = r.n;
SELECT edge.dst FROM (SELECT 1 AS n) AS r JOIN edge ON edge.src = r.n;
SELECT edge.dst FROM (SELECT 1 AS n) AS r, edge WHERE r.n = edge.src;
WITH RECURSIVE r(n, d) AS (SELECT 1, 0 UNION ALL SELECT edge.dst, r.d + 1 FROM edge, r WHERE edge.src = r.n AND r.d < 5) SELECT n, d FROM r;
```

## What differed

```
```
