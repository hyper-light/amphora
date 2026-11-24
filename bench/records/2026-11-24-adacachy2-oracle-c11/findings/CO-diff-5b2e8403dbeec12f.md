# diff Panic: internal error: entered unreachable code: lag/lead arg[0] must be a buffer column reference after planner rewrite, got Some(FunctionCall { name: Name { quote: None, value: "unixepoch" }, distinctness: None, args: [Unary(Negative, Literal(Numeric("100")))], order_by: [], within_group: [], filter_over: FunctionTail { filter_clause: None, over_clause: None } })

seed 8003, state 11309

## Statements

```sql
CREATE TABLE t0(c0 INTEGER COLLATE BINARY PRIMARY KEY, c1 BOOLEAN, c2 , c3 INT);
```

## Queries

```sql
SELECT 1 FROM t0 AS a0 ORDER BY NULL, NULL, NULL, lag(NULL, 1, NULL) OVER (ORDER BY a0.rowid), lag(unixepoch(-100)) OVER (PARTITION BY (NULL % '10') ORDER BY a0.rowid), typeof(lag(unixepoch(-100)) OVER (PARTITION BY (NULL % '10') ORDER BY a0.rowid)) LIMIT 2 OFFSET 2;
```

## What differed

```
internal error: entered unreachable code: lag/lead arg[0] must be a buffer column reference after planner rewrite, got Some(FunctionCall { name: Name { quote: None, value: "unixepoch" }, distinctness: None, args: [Unary(Negative, Literal(Numeric("100")))], order_by: [], within_group: [], filter_over: FunctionTail { filter_clause: None, over_clause: None } })
```
