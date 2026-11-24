# diff Panic: internal error: entered unreachable code: first_value/nth_value arg[0] must be a subquery column ref after planner rewrite, got Some(FunctionCall { name: Name { quote: None, value: "datetime" }, distinctness: None, args: [Literal(Null), Column { database: None, table: TableInternalId(1), column: 0, is_rowid_alias: false }, Column { database: None, table: TableInternalId(1), column: 0, is_rowid_alias: false }], order_by: [], within_group: [], filter_over: FunctionTail { filter_clause: None, over_clause: None } })

seed 8001, state 16701

## Statements

```sql
CREATE TABLE t0(c0 REAL);
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 ORDER BY NULL, NULL, NULL, cume_dist() OVER (ORDER BY NULL DESC), nth_value(datetime(NULL, a0.c0, a0.c0), 3) OVER (ORDER BY a0.c0 DESC, a0.c0, a0.rowid DESC), typeof(nth_value(datetime(NULL, a0.c0, a0.c0), 3) OVER (ORDER BY a0.c0 DESC, a0.c0, a0.rowid DESC)) LIMIT 2 OFFSET 2;
```

## What differed

```
internal error: entered unreachable code: first_value/nth_value arg[0] must be a subquery column ref after planner rewrite, got Some(FunctionCall { name: Name { quote: None, value: "datetime" }, distinctness: None, args: [Literal(Null), Column { database: None, table: TableInternalId(1), column: 0, is_rowid_alias: false }, Column { database: None, table: TableInternalId(1), column: 0, is_rowid_alias: false }], order_by: [], within_group: [], filter_over: FunctionTail { filter_clause: None, over_clause: None } })
```
